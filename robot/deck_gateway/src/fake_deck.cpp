// fake_deck: a scriptable stand-in for the Deck app, for smoke tests and launch_testing.
// Speaks the real interface (Protobuf on rj/v1/*). Prints one line per result, e.g.
//   STATE state=OK owner=1a2b3c4d echo_seq=120 rtt_ms=0.4 stop_reason=
//   CLEAR ok=1 reason=
//   SUMMARY states=STOPPED>OK>NO_CONTROL rtt_ms_avg=0.5 rtt_ms_max=1.2 states_rx=150
//
// Usage: fake_deck [--endpoint E] [--session HEX] <command> [args]
//   info                                   query rj/v1/info
//   drive [seconds=2] [ly=1.0] [rx=0.0]     neutral 0.3 s, clear_stop, L2 (left trigger) + sticks, release
//   hold  [seconds=2] [buttons=HEX]         L2 held + that button mask (no clear) for a while
//   stop                                    publish rj/v1/stop
//   clear [ly=0] [lt=0]                     hold that stick/trigger 0.3 s, then clear_stop
//   gap   [before=1] [silence_ms=500] [after=1]  drive, go silent, then drive again (REARM/link_lost)
//   watch [seconds=2]                       print RobotState changes only
// Default endpoint: the router's local TCP endpoint (no TLS), tcp/127.0.0.1:7451.

#include <atomic>
#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <mutex>
#include <random>
#include <string>
#include <thread>
#include <vector>

#include <zenoh.hxx>

#include "remote_joy/v1/control.pb.h"
#include "remote_joy/v1/info.pb.h"
#include "remote_joy_interface/keys.hpp"

namespace v1 = remote_joy::v1;
namespace keys = remote_joy::v1::keys;
using namespace std::chrono_literals;

namespace {

std::uint64_t now_us() {
  return static_cast<std::uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(
    std::chrono::steady_clock::now().time_since_epoch()).count());
}

zenoh::KeyExpr ke(std::string_view s) { return zenoh::KeyExpr(std::string(s)); }

const char * state_name(int s) {
  switch (s) {
    case v1::RobotState::OK: return "OK";
    case v1::RobotState::STOPPED: return "STOPPED";
    case v1::RobotState::NO_CONTROL: return "NO_CONTROL";
    case v1::RobotState::BUSY: return "BUSY";
    case v1::RobotState::REARM: return "REARM";
    default: return "UNSPECIFIED";
  }
}

double arg_or(const std::vector<std::string> & a, std::size_t i, double dflt) {
  return i < a.size() ? std::atof(a[i].c_str()) : dflt;
}

class FakeDeck {
public:
  FakeDeck(const std::string & endpoint, std::uint32_t session_id) : session_id_(session_id) {
    auto c = zenoh::Config::create_default();
    c.insert_json5("mode", "\"client\"");
    c.insert_json5("connect/endpoints", "[\"" + endpoint + "\"]");
    c.insert_json5("scouting/multicast/enabled", "false");
    session_.emplace(zenoh::Session::open(std::move(c)));
    zenoh::Session::PublisherOptions o;
    o.congestion_control = Z_CONGESTION_CONTROL_DROP;
    o.priority = Z_PRIORITY_REAL_TIME;
    o.is_express = true;
    o.reliability = Z_RELIABILITY_BEST_EFFORT;
    o.encoding = zenoh::Encoding(v1::encoding("Control"));
    control_pub_.emplace(session_->declare_publisher(ke(keys::kControl), std::move(o)));
    char tok[9];
    std::snprintf(tok, sizeof(tok), "%08x", session_id_);
    token_.emplace(session_->liveliness_declare_token(ke(std::string(keys::kDeckPrefix) + "/" + tok)));
    state_sub_.emplace(session_->declare_subscriber(ke(keys::kRobotState),
      [this](const zenoh::Sample & s) { on_state(s); }, [] {}));
    std::printf("SESSION %08x\n", session_id_);
    std::this_thread::sleep_for(300ms);  // let declarations propagate
  }

  ~FakeDeck() {
    // Deliberate disconnect, like the app: the token goes with the session.
    token_.reset();
    state_sub_.reset();
    control_pub_.reset();
    session_.reset();
  }

  void send(std::uint32_t buttons, float ly = 0, float rx = 0, bool stop = false, float lt = 0) {
    v1::Control c;
    c.set_session_id(session_id_);
    c.set_seq(++seq_);
    c.set_deck_time_us(now_us());
    const float axes[6] = {0, ly, rx, 0, lt, 0};
    for (float a : axes) {
      c.add_axes(a);
    }
    c.set_buttons(buttons);
    c.set_stop(stop);
    control_pub_->put(zenoh::Bytes(c.SerializeAsString()));
  }

  /// 50 Hz for `seconds`.
  /// 50 Hz for `seconds`. `deadman` holds the left trigger (the gateway's default deadman).
  void stream(double seconds, std::uint32_t buttons, float ly = 0, float rx = 0, bool deadman = false) {
    const auto end = std::chrono::steady_clock::now() + std::chrono::duration<double>(seconds);
    auto next = std::chrono::steady_clock::now();
    while (std::chrono::steady_clock::now() < end) {
      send(buttons, ly, rx, false, deadman ? 1.0f : 0.0f);
      next += 20ms;
      std::this_thread::sleep_until(next);
    }
  }

  /// clear_stop while still holding `ly` / the deadman (to provoke refusals).
  bool clear(float ly = 0, bool deadman = false) {
    v1::ClearStopRequest req;
    req.set_session_id(session_id_);
    req.set_released_seq(seq_ + 1);
    send(0, ly, 0, false, deadman ? 1.0f : 0.0f);  // the first seq with stop=false <= released_seq
    zenoh::Session::GetOptions opts;
    opts.payload = zenoh::Bytes(req.SerializeAsString());
    opts.timeout_ms = 2000;
    bool ok = false;
    std::string reason = "timeout";
    auto replies = session_->get(ke(keys::kClearStop), "", zenoh::channels::FifoChannel(4),
      std::move(opts));
    for (auto r = replies.recv(); std::holds_alternative<zenoh::Reply>(r); r = replies.recv()) {
      auto & reply = std::get<zenoh::Reply>(r);
      if (!reply.is_ok()) {
        continue;
      }
      v1::ClearStopReply m;
      const auto v = reply.get_ok().get_payload().as_vector();
      if (m.ParseFromArray(v.data(), static_cast<int>(v.size()))) {
        ok = m.ok();
        reason = m.reason();
      }
    }
    std::printf("CLEAR ok=%d reason=%s\n", ok ? 1 : 0, reason.c_str());
    return ok;
  }

  void info() {
    zenoh::Session::GetOptions opts;
    opts.timeout_ms = 2000;
    auto replies = session_->get(ke(keys::kInfo), "", zenoh::channels::FifoChannel(4),
      std::move(opts));
    bool got = false;
    for (auto r = replies.recv(); std::holds_alternative<zenoh::Reply>(r); r = replies.recv()) {
      auto & reply = std::get<zenoh::Reply>(r);
      if (!reply.is_ok()) {
        continue;
      }
      v1::Info m;
      const auto v = reply.get_ok().get_payload().as_vector();
      if (m.ParseFromArray(v.data(), static_cast<int>(v.size()))) {
        got = true;
        std::string id;
        for (unsigned char ch : m.robot_id()) {
          char b[3];
          std::snprintf(b, sizeof(b), "%02x", ch);
          id += b;
        }
        std::printf("INFO robot_name=%s interface=%u.%u camera=%d actions=%d hints=%d robot_id=%s\n",
          m.robot_name().c_str(), m.interface_version(), m.interface_minor(),
          m.camera().available() ? 1 : 0, m.actions_size(), m.button_hints_size(),
          id.empty() ? "-" : id.c_str());
      }
    }
    if (!got) {
      std::printf("INFO none\n");
    }
  }

  void stop() {
    v1::Stop m;
    m.set_session_id(session_id_);
    m.set_deck_time_us(now_us());
    m.set_source("fake_deck");
    zenoh::Session::PutOptions o;
    o.congestion_control = Z_CONGESTION_CONTROL_BLOCK;
    o.priority = Z_PRIORITY_REAL_TIME;
    o.is_express = true;
    o.reliability = Z_RELIABILITY_RELIABLE;
    session_->put(ke(keys::kStop), zenoh::Bytes(m.SerializeAsString()), std::move(o));
    std::printf("STOP sent\n");
  }

  void summary() {
    std::lock_guard<std::mutex> lock(mutex_);
    std::printf("SUMMARY states=%s rtt_ms_avg=%.2f rtt_ms_max=%.2f states_rx=%llu last_reason=%s\n",
      history_.empty() ? "none" : history_.c_str(),
      rtt_n_ ? rtt_sum_ms_ / static_cast<double>(rtt_n_) : 0.0, rtt_max_ms_,
      static_cast<unsigned long long>(states_rx_), last_reason_.c_str());
    std::fflush(stdout);
  }

private:
  void on_state(const zenoh::Sample & s) {
    v1::RobotState m;
    const auto v = s.get_payload().as_vector();
    if (!m.ParseFromArray(v.data(), static_cast<int>(v.size()))) {
      return;
    }
    std::lock_guard<std::mutex> lock(mutex_);
    ++states_rx_;
    // BUSY is derived on the Deck: someone else owns the robot.
    const bool other = m.owner_session() != 0 && m.owner_session() != session_id_;
    const std::string st = other ? "BUSY" : state_name(m.state());
    if (!other && m.echo_deck_time_us() != 0 && m.echo_seq() != last_echo_) {
      last_echo_ = m.echo_seq();
      const double rtt = static_cast<double>(now_us() - m.echo_deck_time_us()) / 1000.0;
      rtt_sum_ms_ += rtt;
      rtt_max_ms_ = std::max(rtt_max_ms_, rtt);
      ++rtt_n_;
    }
    if (st != last_state_ || m.stop_reason() != last_reason_) {
      std::printf("STATE state=%s owner=%08x echo_seq=%u stop_reason=%s\n", st.c_str(),
        m.owner_session(), m.echo_seq(), m.stop_reason().c_str());
      std::fflush(stdout);
      if (st != last_state_) {
        history_ += (history_.empty() ? "" : ">") + st;
      }
      last_state_ = st;
      last_reason_ = m.stop_reason();
    }
  }

  std::uint32_t session_id_;
  std::uint32_t seq_ = 0;
  std::mutex mutex_;
  std::string last_state_, last_reason_, history_;
  std::uint32_t last_echo_ = 0;
  double rtt_sum_ms_ = 0, rtt_max_ms_ = 0;
  std::uint64_t rtt_n_ = 0, states_rx_ = 0;

  std::optional<zenoh::Session> session_;
  std::optional<zenoh::Publisher> control_pub_;
  std::optional<zenoh::LivelinessToken> token_;
  std::optional<zenoh::Subscriber<void>> state_sub_;
};

}  // namespace

int main(int argc, char ** argv) {
  std::string endpoint = "tcp/127.0.0.1:7451";
  std::uint32_t session = std::random_device{}() | 1u;
  std::vector<std::string> args;
  for (int i = 1; i < argc; ++i) {
    const std::string a = argv[i];
    if (a == "--endpoint" && i + 1 < argc) {
      endpoint = argv[++i];
    } else if (a == "--session" && i + 1 < argc) {
      session = static_cast<std::uint32_t>(std::strtoul(argv[++i], nullptr, 16));
    } else {
      args.push_back(a);
    }
  }
  if (args.empty()) {
    std::fprintf(stderr, "usage: fake_deck [--endpoint E] [--session HEX] info|drive|hold|stop|clear|gap|watch [args]\n");
    return 2;
  }
  const std::string cmd = args[0];
  try {
    FakeDeck deck(endpoint, session);
    if (cmd == "info") {
      deck.info();
    } else if (cmd == "drive") {
      const double secs = arg_or(args, 1, 2.0);
      const auto ly = static_cast<float>(arg_or(args, 2, 1.0));
      const auto rx = static_cast<float>(arg_or(args, 3, 0.0));
      deck.stream(0.3, 0);
      deck.clear();
      deck.stream(secs, 0, ly, rx, true);
      deck.stream(0.5, 0);
    } else if (cmd == "hold") {
      const double secs = arg_or(args, 1, 2.0);
      const auto buttons = args.size() > 2
        ? static_cast<std::uint32_t>(std::strtoul(args[2].c_str(), nullptr, 16)) : 0u;
      deck.stream(secs, buttons, 1.0f, 0, true);
    } else if (cmd == "stop") {
      deck.stop();
      std::this_thread::sleep_for(300ms);
    } else if (cmd == "clear") {
      // Optional stick / trigger while clearing, to provoke the refusals.
      const auto ly = static_cast<float>(arg_or(args, 1, 0.0));
      const bool deadman = arg_or(args, 2, 0.0) > 0.5;
      deck.stream(0.3, 0, ly, 0, deadman);
      deck.clear(ly, deadman);
      deck.stream(0.3, 0);
    } else if (cmd == "gap") {
      const double before = arg_or(args, 1, 1.0);
      const double silence_ms = arg_or(args, 2, 500.0);
      const double after = arg_or(args, 3, 1.0);
      deck.stream(0.3, 0);
      deck.clear();
      deck.stream(before, 0, 1.0f, 0, true);
      std::this_thread::sleep_for(std::chrono::duration<double, std::milli>(silence_ms));
      deck.stream(0.2, 0, 1.0f, 0, true);  // still holding the deadman when the link returns
      deck.stream(0.2, 0);         // release
      deck.stream(after, 0, 1.0f, 0, true);
      deck.stream(0.3, 0);
    } else if (cmd == "watch") {
      std::this_thread::sleep_for(std::chrono::duration<double>(arg_or(args, 1, 2.0)));
    } else {
      std::fprintf(stderr, "unknown command %s\n", cmd.c_str());
      return 2;
    }
    std::this_thread::sleep_for(300ms);
    deck.summary();
  } catch (const std::exception & e) {
    std::fprintf(stderr, "fake_deck: %s\n", e.what());
    return 1;
  }
  return 0;
}
