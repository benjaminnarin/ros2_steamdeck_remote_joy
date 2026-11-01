// deck_gateway: Deck interface (Zenoh + Protobuf) <-> ROS. All safety decisions are made by
// deck_gateway_core::SafetyCore; this node only translates and publishes. Spec: docs/robot.md.
//
// Threads: Zenoh callbacks arrive on Zenoh's threads, the 50 Hz tick on the executor thread.
// Every access to the core goes through mutex_.

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <utility>

#include <diagnostic_updater/diagnostic_updater.hpp>
#include <rclcpp/rclcpp.hpp>
#include <sensor_msgs/msg/joy.hpp>
#include <std_msgs/msg/bool.hpp>
#include <std_srvs/srv/trigger.hpp>
#include <zenoh.hxx>

#include "deck_gateway/core/safety_core.hpp"
#include "remote_joy/v1/control.pb.h"
#include "remote_joy/v1/info.pb.h"
#include "remote_joy_interface/keys.hpp"

namespace deck_gateway {

namespace v1 = remote_joy::v1;
namespace keys = remote_joy::v1::keys;
using core::Micros;

namespace {

Micros steady_now_us() {
  return std::chrono::duration_cast<std::chrono::microseconds>(
    std::chrono::steady_clock::now().time_since_epoch()).count();
}

zenoh::KeyExpr ke(std::string_view s) { return zenoh::KeyExpr(std::string(s)); }

template <typename Msg>
bool parse(const zenoh::Bytes & b, Msg & msg) {
  const std::vector<uint8_t> v = b.as_vector();
  return msg.ParseFromArray(v.data(), static_cast<int>(v.size()));
}

template <typename Msg>
zenoh::Bytes serialize(const Msg & msg) { return zenoh::Bytes(msg.SerializeAsString()); }

/// "rj/v1/deck/0001eb13" -> 0x0001eb13
std::optional<std::uint32_t> session_from_token(std::string_view key) {
  const auto slash = key.rfind('/');
  if (slash == std::string_view::npos || key.size() - slash - 1 != 8) {
    return std::nullopt;
  }
  std::uint32_t id = 0;
  for (char ch : key.substr(slash + 1)) {
    id <<= 4;
    if (ch >= '0' && ch <= '9') {
      id |= static_cast<std::uint32_t>(ch - '0');
    } else if (ch >= 'a' && ch <= 'f') {
      id |= static_cast<std::uint32_t>(ch - 'a' + 10);
    } else {
      return std::nullopt;
    }
  }
  return id;
}

std::string hex(std::uint32_t id) {
  char buf[9];
  std::snprintf(buf, sizeof(buf), "%08x", id);
  return buf;
}

core::ControlIn to_core(const v1::Control & c) {
  core::ControlIn in;
  in.session_id = c.session_id();
  in.seq = c.seq();
  in.deck_time_us = c.deck_time_us();
  in.axes_count = std::min<std::size_t>(static_cast<std::size_t>(c.axes_size()), in.axes.size());
  for (std::size_t i = 0; i < in.axes_count; ++i) {
    in.axes[i] = c.axes(static_cast<int>(i));
  }
  in.buttons = c.buttons();
  in.stop = c.stop();
  return in;
}

}  // namespace

class GatewayNode : public rclcpp::Node {
public:
  GatewayNode()
  : rclcpp::Node("deck_gateway"),
    core_(load_config(), steady_now_us()),
    diagnostics_(this)
  {
    robot_name_ = declare_parameter<std::string>("robot_name", "robot");
    const auto endpoint = declare_parameter<std::string>("zenoh_endpoint", "tcp/127.0.0.1:7451");
    const double state_rate_hz = declare_parameter<double>("state_rate_hz", 5.0);
    state_every_ticks_ = std::max(1, static_cast<int>(std::lround(50.0 / state_rate_hz)));

    joy_pub_ = create_publisher<sensor_msgs::msg::Joy>("joy", 10);
    const auto latched = rclcpp::QoS(1).reliable().transient_local();
    stop_latched_pub_ = create_publisher<std_msgs::msg::Bool>("steamdeck/stop_latched", latched);
    owner_pub_ = create_publisher<std_msgs::msg::Bool>("steamdeck/owner_connected", latched);
    set_stop_srv_ = create_service<std_srvs::srv::Trigger>("steamdeck/set_stop",
      [this](const std::shared_ptr<std_srvs::srv::Trigger::Request>,
             std::shared_ptr<std_srvs::srv::Trigger::Response> res) {
        std::lock_guard<std::mutex> lock(mutex_);
        apply(core_.set_stop("service", steady_now_us()));
        res->success = true;
        res->message = "stop latched";
      });

    diagnostics_.setHardwareID("deck_gateway");
    diagnostics_.add("link", this, &GatewayNode::diagnose);

    open_zenoh(endpoint);
    {
      std::lock_guard<std::mutex> lock(mutex_);
      publish_bools(true);
    }
    tick_timer_ = create_wall_timer(std::chrono::milliseconds(20), [this] { on_tick(); });
    RCLCPP_INFO(get_logger(), "deck_gateway '%s' up: zenoh %s, stop latched (startup)",
      robot_name_.c_str(), endpoint.c_str());
  }

private:
  core::Config load_config() {
    core::Config c;
    c.watchdog_us = declare_parameter<int>("watchdog_ms", 250) * 1000;
    c.resume_grace_us = declare_parameter<int>("resume_grace_ms", 1000) * 1000;
    c.owner_timeout_us = declare_parameter<int>("owner_timeout_ms", 2000) * 1000;
    c.max_control_delay_us = declare_parameter<int>("max_control_delay_ms", 150) * 1000;
    c.clear_stop_stick_tolerance =
      static_cast<float>(declare_parameter<double>("clear_stop_stick_tolerance", 0.1));
    const auto deadman = declare_parameter<std::string>("deadman", "LT");
    const auto parsed = core::parse_deadman(deadman);
    if (!parsed) {
      throw std::invalid_argument("deadman must be LT, RT, LB or RB (got '" + deadman + "')");
    }
    c.deadman = *parsed;
    if (c.deadman.kind == core::Deadman::Kind::kButton && c.deadman.index == core::kButtonRB) {
      c.needs_deadman_buttons = 0;  // RB is the deadman itself; there's no separate turbo gate
    }
    RCLCPP_INFO(get_logger(), "deadman: %s (published as /joy button %zu)", deadman.c_str(),
      core::kJoyDeadmanButton);
    return c;
  }

  void open_zenoh(const std::string & endpoint) {
    auto config = zenoh::Config::create_default();
    config.insert_json5("mode", "\"client\"");
    config.insert_json5("connect/endpoints", "[\"" + endpoint + "\"]");
    config.insert_json5("scouting/multicast/enabled", "false");
    // Keep retrying if the router restarts (it's respawned by the launch file).
    config.insert_json5("connect/exit_on_failure", "false");
    config.insert_json5("connect/timeout_ms", "-1");
    config.insert_json5("connect/retry",
      "{period_init_ms: 200, period_max_ms: 1000, period_increase_factor: 1.5}");
    config.insert_json5("transport/link/tx/lease", "2000");
    config.insert_json5("transport/link/tx/keep_alive", "4");
    session_.emplace(zenoh::Session::open(std::move(config)));

    zenoh::Session::PublisherOptions state_opts;
    state_opts.congestion_control = Z_CONGESTION_CONTROL_DROP;
    state_opts.priority = Z_PRIORITY_REAL_TIME;
    state_opts.is_express = true;
    state_opts.reliability = Z_RELIABILITY_BEST_EFFORT;
    state_opts.encoding = zenoh::Encoding(v1::encoding("RobotState"));
    state_pub_.emplace(session_->declare_publisher(ke(keys::kRobotState), std::move(state_opts)));

    control_sub_.emplace(session_->declare_subscriber(ke(keys::kControl),
      [this](const zenoh::Sample & s) { on_control(s); }, [] {}));
    stop_sub_.emplace(session_->declare_subscriber(ke(keys::kStop),
      [this](const zenoh::Sample & s) { on_stop(s); }, [] {}));
    zenoh::Session::LivelinessSubscriberOptions tok_opts;
    tok_opts.history = true;
    token_sub_.emplace(session_->liveliness_declare_subscriber(ke(keys::kDeckAll),
      [this](const zenoh::Sample & s) { on_token(s); }, [] {}, std::move(tok_opts)));
    clear_q_.emplace(session_->declare_queryable(ke(keys::kClearStop),
      [this](const zenoh::Query & q) { on_clear_stop(q); }, [] {}));
    info_q_.emplace(session_->declare_queryable(ke(keys::kInfo),
      [this](const zenoh::Query & q) { on_info(q); }, [] {}));
  }

  // ── Zenoh callbacks ──────────────────────────────────────────
  void on_control(const zenoh::Sample & s) {
    v1::Control c;
    if (!parse(s.get_payload(), c)) {
      ++bad_messages_;
      return;
    }
    const Micros now = steady_now_us();
    std::lock_guard<std::mutex> lock(mutex_);
    apply(core_.on_control(to_core(c), now));
  }

  void on_stop(const zenoh::Sample & s) {
    v1::Stop m;
    if (!parse(s.get_payload(), m)) {
      ++bad_messages_;
      return;
    }
    std::lock_guard<std::mutex> lock(mutex_);
    apply(core_.on_stop(m.session_id(), m.source().empty() ? "stop" : m.source(), steady_now_us()));
  }

  void on_token(const zenoh::Sample & s) {
    const auto key = s.get_keyexpr().as_string_view();
    const auto id = session_from_token(key);
    if (!id) {
      RCLCPP_WARN(get_logger(), "ignoring malformed Deck token %.*s",
        static_cast<int>(key.size()), key.data());
      return;
    }
    if (s.get_kind() == Z_SAMPLE_KIND_PUT) {
      RCLCPP_INFO(get_logger(), "[%s] Deck connected (liveliness token)", hex(*id).c_str());
      return;
    }
    RCLCPP_INFO(get_logger(), "[%s] Deck gone (liveliness token withdrawn)", hex(*id).c_str());
    std::lock_guard<std::mutex> lock(mutex_);
    apply(core_.on_token_lost(*id, steady_now_us()));
  }

  void on_clear_stop(const zenoh::Query & q) {
    v1::ClearStopRequest req;
    v1::ClearStopReply reply;
    const auto payload = q.get_payload();
    if (!payload || !parse(payload->get(), req)) {
      reply.set_ok(false);
      reply.set_reason("bad_request");
    } else {
      std::lock_guard<std::mutex> lock(mutex_);
      auto r = core_.clear_stop(req.session_id(), req.released_seq(), steady_now_us());
      reply.set_ok(r.ok);
      reply.set_reason(r.reason);
      core::Step step;
      step.events = std::move(r.events);
      apply(step);
      if (!r.ok) {
        RCLCPP_INFO(get_logger(), "[%s] clear_stop refused: %s",
          hex(req.session_id()).c_str(), r.reason.c_str());
      }
    }
    zenoh::Query::ReplyOptions opts;
    opts.encoding = zenoh::Encoding(v1::encoding("ClearStopReply"));
    q.reply(ke(keys::kClearStop), serialize(reply), std::move(opts));
  }

  void on_info(const zenoh::Query & q) {
    v1::Info info;
    info.set_robot_name(robot_name_);
    info.set_interface_version(v1::kInterfaceVersion);
    info.set_interface_minor(v1::kInterfaceMinor);
    info.mutable_camera()->set_available(false);  // deck_video (M2) will fill this in
    // robot_id (SHA-256 of the robot CA) comes with the pair tool.
    zenoh::Query::ReplyOptions opts;
    opts.encoding = zenoh::Encoding(v1::encoding("Info"));
    q.reply(ke(keys::kInfo), serialize(info), std::move(opts));
  }

  // ── 50 Hz tick ───────────────────────────────────────────────
  void on_tick() {
    std::lock_guard<std::mutex> lock(mutex_);
    apply(core_.tick(steady_now_us()));
    if (++ticks_ % state_every_ticks_ == 0) {
      publish_state();
    }
  }

  // ── output (called with mutex_ held) ─────────────────────────
  void apply(const core::Step & step) {
    for (const auto & e : step.events) {
      RCLCPP_INFO(get_logger(), "[%s] %s%s%s", hex(e.session).c_str(), core::to_string(e.type),
        e.detail.empty() ? "" : ": ", e.detail.c_str());
    }
    if (step.joy) {
      sensor_msgs::msg::Joy joy;
      joy.header.stamp = now();
      joy.header.frame_id = "steamdeck";
      joy.axes.assign(step.joy->axes.begin(), step.joy->axes.end());
      joy.buttons.assign(step.joy->buttons.begin(), step.joy->buttons.end());
      joy_pub_->publish(joy);
    }
    if (step.publish_state) {
      publish_state();
    }
    publish_bools(false);
  }

  void publish_state() {
    const auto rs = core_.robot_state();
    v1::RobotState m;
    m.set_robot_time_us(static_cast<std::uint64_t>(steady_now_us()));
    m.set_state(static_cast<v1::RobotState::State>(rs.state));
    m.set_owner_session(rs.owner_session);
    m.set_echo_seq(rs.echo_seq);
    m.set_echo_deck_time_us(rs.echo_deck_time_us);
    m.set_stop_reason(rs.stop_reason);
    state_pub_->put(serialize(m));
  }

  void publish_bools(bool force) {
    const bool latched = core_.stop_latched();
    const bool owner = core_.owner_connected();
    if (force || latched != last_latched_) {
      stop_latched_pub_->publish(std_msgs::msg::Bool().set__data(latched));
      last_latched_ = latched;
    }
    if (force || owner != last_owner_) {
      owner_pub_->publish(std_msgs::msg::Bool().set__data(owner));
      last_owner_ = owner;
    }
  }

  void diagnose(diagnostic_updater::DiagnosticStatusWrapper & stat) {
    std::lock_guard<std::mutex> lock(mutex_);
    const auto rs = core_.robot_state();
    const auto & c = core_.counters();
    const auto now = steady_now_us();
    const double dt = last_diag_us_ > 0 ? static_cast<double>(now - last_diag_us_) / 1e6 : 0.0;
    const double rate = dt > 0 ? static_cast<double>(c.controls_applied - last_applied_) / dt : 0.0;
    last_diag_us_ = now;
    last_applied_ = c.controls_applied;

    using diagnostic_msgs::msg::DiagnosticStatus;
    if (!core_.owner_connected()) {
      stat.summary(DiagnosticStatus::OK, "no Deck in control");
    } else if (rs.state == core::State::kOk) {
      stat.summary(DiagnosticStatus::OK, "driving");
    } else {
      stat.summaryf(DiagnosticStatus::WARN, "%s%s%s", core::to_string(rs.state),
        rs.stop_reason.empty() ? "" : ": ", rs.stop_reason.c_str());
    }
    stat.add("state", core::to_string(rs.state));
    stat.add("stop_reason", rs.stop_reason);
    stat.add("owner_session", core_.owner_connected() ? hex(rs.owner_session) : std::string("none"));
    stat.addf("control_rate_hz", "%.1f", rate);
    stat.add("controls_late", c.controls_late);
    stat.add("controls_out_of_order", c.controls_out_of_order);
    stat.add("controls_from_other_decks", c.controls_from_others);
    stat.add("watchdog_trips", c.watchdog_trips);
    stat.add("rearms", c.rearms);
    stat.add("link_lost", c.link_lost);
    stat.add("owner_changes", c.owner_changes);
    stat.add("stops_latched", c.stops_latched);
    stat.add("stops_cleared", c.stops_cleared);
    stat.add("bad_messages", bad_messages_.load());
  }

  std::mutex mutex_;
  core::SafetyCore core_;
  std::string robot_name_;
  int state_every_ticks_ = 10;
  std::uint64_t ticks_ = 0;
  bool last_latched_ = true;
  bool last_owner_ = false;
  Micros last_diag_us_ = 0;
  std::uint64_t last_applied_ = 0;
  std::atomic<std::uint64_t> bad_messages_{0};

  rclcpp::Publisher<sensor_msgs::msg::Joy>::SharedPtr joy_pub_;
  rclcpp::Publisher<std_msgs::msg::Bool>::SharedPtr stop_latched_pub_;
  rclcpp::Publisher<std_msgs::msg::Bool>::SharedPtr owner_pub_;
  rclcpp::Service<std_srvs::srv::Trigger>::SharedPtr set_stop_srv_;
  rclcpp::TimerBase::SharedPtr tick_timer_;
  diagnostic_updater::Updater diagnostics_;

  // Declared last so they're destroyed first: no callback runs into a half-destroyed node.
  std::optional<zenoh::Session> session_;
  std::optional<zenoh::Publisher> state_pub_;
  std::optional<zenoh::Subscriber<void>> control_sub_;
  std::optional<zenoh::Subscriber<void>> stop_sub_;
  std::optional<zenoh::Subscriber<void>> token_sub_;
  std::optional<zenoh::Queryable<void>> clear_q_;
  std::optional<zenoh::Queryable<void>> info_q_;
};

}  // namespace deck_gateway

int main(int argc, char ** argv) {
  GOOGLE_PROTOBUF_VERIFY_VERSION;
  rclcpp::init(argc, argv);
  int rc = 0;
  try {
    rclcpp::spin(std::make_shared<deck_gateway::GatewayNode>());
  } catch (const std::exception & e) {
    RCLCPP_FATAL(rclcpp::get_logger("deck_gateway"), "%s", e.what());
    rc = 1;
  }
  rclcpp::shutdown();
  google::protobuf::ShutdownProtobufLibrary();
  return rc;
}
