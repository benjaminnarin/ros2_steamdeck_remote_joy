// deck_video::VideoPublisher: sensor_msgs/Image -> JPEG -> rj/v1/video (remote_joy.v1.VideoFrame).
// Spec: docs/robot.md § deck_video, docs/interface.md (rj/v1/video).
//
// - On demand: a Zenoh matching listener tells us whether any Deck subscribes to rj/v1/video.
//   Only then do we subscribe to the camera. It sets a flag; the timer (executor thread) creates
//   or drops the ROS subscription.
// - No stale frames: sensor-data QoS, depth 1; the timer takes only the newest image, and frames
//   that can't keep up are skipped.
// - Adaptive rate: Decks report what they receive on rj/v1/video_feedback, and RateController
//   (rate_control.hpp) steps fps, quality and size down when frames queue or get lost, and back
//   up when the link recovers. A Deck that sends no feedback gets the configured rate.

#include <atomic>
#include <chrono>
#include <memory>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include <cv_bridge/cv_bridge.hpp>
#include <opencv2/imgcodecs.hpp>
#include <opencv2/imgproc.hpp>
#include <rclcpp/rclcpp.hpp>
#include <rclcpp_components/register_node_macro.hpp>
#include <sensor_msgs/msg/image.hpp>
#include <zenoh.hxx>

#include "deck_video/frame_math.hpp"
#include "deck_video/rate_control.hpp"
#include "remote_joy/v1/video.pb.h"
#include "remote_joy_interface/keys.hpp"

namespace deck_video {

namespace v1 = remote_joy::v1;
namespace keys = remote_joy::v1::keys;

namespace {
std::int64_t steady_us() {
  return std::chrono::duration_cast<std::chrono::microseconds>(
    std::chrono::steady_clock::now().time_since_epoch()).count();
}
constexpr std::size_t kMaxFrameBytes = 512 * 1024;  // the router drops bigger video puts
}  // namespace

class VideoPublisher : public rclcpp::Node {
public:
  explicit VideoPublisher(const rclcpp::NodeOptions & options)
  : rclcpp::Node("deck_video", options)
  {
    image_topic_ = declare_parameter<std::string>("image_topic", "/camera/image_raw");
    max_width_ = static_cast<int>(declare_parameter<int>("max_width", 480));
    fps_ = declare_parameter<double>("fps", 30.0);
    quality_ = static_cast<int>(declare_parameter<int>("jpeg_quality", 70));
    adaptive_ = declare_parameter<bool>("adaptive", true);
    AdaptConfig ac;
    ac.high_delay_us = declare_parameter<int>("adapt_high_delay_ms", 150) * 1000;
    ac.low_delay_us = declare_parameter<int>("adapt_low_delay_ms", 75) * 1000;
    rate_ = RateController(ac);
    const auto endpoint = declare_parameter<std::string>("zenoh_endpoint", "tcp/127.0.0.1:7451");

    auto config = zenoh::Config::create_default();
    config.insert_json5("mode", "\"client\"");
    config.insert_json5("connect/endpoints", "[\"" + endpoint + "\"]");
    config.insert_json5("scouting/multicast/enabled", "false");
    config.insert_json5("connect/exit_on_failure", "false");
    config.insert_json5("connect/timeout_ms", "-1");
    config.insert_json5("connect/retry",
      "{period_init_ms: 200, period_max_ms: 1000, period_increase_factor: 1.5}");
    session_.emplace(zenoh::Session::open(std::move(config)));

    zenoh::Session::PublisherOptions o;
    o.congestion_control = Z_CONGESTION_CONTROL_DROP;
    o.priority = Z_PRIORITY_INTERACTIVE_LOW;
    o.reliability = Z_RELIABILITY_BEST_EFFORT;
    o.encoding = zenoh::Encoding(v1::encoding("VideoFrame"));
    pub_.emplace(session_->declare_publisher(zenoh::KeyExpr(std::string(keys::kVideo)), std::move(o)));
    matching_.emplace(pub_->declare_matching_listener(
      [this](const zenoh::MatchingStatus & st) { wanted_.store(st.matching); }, [] {}));
    wanted_.store(pub_->get_matching_status().matching);
    if (adaptive_) {
      feedback_sub_.emplace(session_->declare_subscriber(
        zenoh::KeyExpr(std::string(keys::kVideoFeedback)),
        [this](const zenoh::Sample & s) { on_feedback(s); }, [] {}));
    }

    const auto period = std::chrono::duration<double>(1.0 / std::max(1.0, fps_ * 2.0));
    timer_ = create_wall_timer(std::chrono::duration_cast<std::chrono::nanoseconds>(period),
      [this] { on_timer(); });
    stats_timer_ = create_wall_timer(std::chrono::seconds(5), [this] { report(); });
    RCLCPP_INFO(get_logger(), "deck_video: %s -> %s (max %d px wide, %.0f fps, quality %d), on demand%s",
      image_topic_.c_str(), std::string(keys::kVideo).c_str(), max_width_, fps_, quality_,
      adaptive_ ? ", adaptive" : "");
  }

private:
  void on_feedback(const zenoh::Sample & s) {
    const auto now = steady_us();
    const std::vector<uint8_t> v = s.get_payload().as_vector();
    v1::VideoFeedback m;
    if (!m.ParseFromArray(v.data(), static_cast<int>(v.size()))) {
      return;
    }
    std::lock_guard<std::mutex> lock(rate_mutex_);
    rate_.on_feedback(Feedback{m.session_id(), m.last_frame_id(), m.last_frame_age_us(), m.frames_received()},
      now);
  }

  /// The rate to send at now, after letting the controller react. Logs level changes.
  Rate current_rate(std::int64_t now, bool watching) {
    std::lock_guard<std::mutex> lock(rate_mutex_);
    if (!adaptive_) {
      return level_rate(0, fps_, quality_);
    }
    if (watching) {
      rate_.tick(now);
    } else if (was_watching_) {
      rate_.reset_sessions(now);
    }
    was_watching_ = watching;
    const int level = rate_.level();
    const auto r = level_rate(level, fps_, quality_);
    if (level != logged_level_) {
      const auto d = rate_.last_delay_us();
      RCLCPP_INFO(get_logger(), "video level %d -> %d: %.0f fps, quality %d, %.0f%% size (delay %s)",
        logged_level_, level, r.fps, r.quality, r.width_scale * 100.0,
        d ? (std::to_string(*d / 1000) + " ms").c_str() : "unknown");
      logged_level_ = level;
    }
    return r;
  }

  void on_image(sensor_msgs::msg::Image::ConstSharedPtr msg) {
    const auto arrival = steady_us();
    std::lock_guard<std::mutex> lock(mutex_);
    latest_ = msg;
    latest_arrival_us_ = arrival;
    ++images_in_;
  }

  void update_subscription() {
    const bool wanted = wanted_.load();
    if (wanted && !sub_) {
      sub_ = create_subscription<sensor_msgs::msg::Image>(image_topic_,
        rclcpp::SensorDataQoS().keep_last(1),
        [this](sensor_msgs::msg::Image::ConstSharedPtr m) { on_image(std::move(m)); });
      RCLCPP_INFO(get_logger(), "a Deck is watching: subscribed to %s", image_topic_.c_str());
    } else if (!wanted && sub_) {
      sub_.reset();
      std::lock_guard<std::mutex> lock(mutex_);
      latest_.reset();
      RCLCPP_INFO(get_logger(), "no Deck watching: unsubscribed from %s", image_topic_.c_str());
    }
  }

  void on_timer() {
    update_subscription();
    const auto now = steady_us();
    const auto rate = current_rate(now, sub_ != nullptr);
    if (!sub_) {
      return;
    }
    if (!due(now, last_sent_us_, rate.fps)) {
      return;
    }
    sensor_msgs::msg::Image::ConstSharedPtr img;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      img = latest_;
      latest_.reset();  // each image is sent at most once
    }
    if (!img) {
      return;
    }
    send(img, now, rate);
  }

  void send(const sensor_msgs::msg::Image::ConstSharedPtr & img, std::int64_t now, const Rate & rate) {
    const auto capture_us = capture_steady_us(now, this->now().nanoseconds(),
      rclcpp::Time(img->header.stamp).nanoseconds());
    cv_bridge::CvImageConstPtr cv;
    try {
      cv = cv_bridge::toCvShare(img, "bgr8");
    } catch (const std::exception & e) {
      RCLCPP_WARN_THROTTLE(get_logger(), *get_clock(), 5000, "can't convert %s image: %s",
        img->encoding.c_str(), e.what());
      return;
    }
    int width_limit = max_width_;
    if (rate.width_scale < 1.0) {
      const int base = max_width_ > 0 ? std::min(max_width_, cv->image.cols) : cv->image.cols;
      width_limit = std::max(2, static_cast<int>(base * rate.width_scale));
    }
    const auto [w, h] = scaled_size(cv->image.cols, cv->image.rows, width_limit);
    cv::Mat out;
    if (w != cv->image.cols || h != cv->image.rows) {
      cv::resize(cv->image, out, cv::Size(w, h), 0, 0, cv::INTER_AREA);
    } else {
      out = cv->image;
    }
    std::vector<uchar> jpeg;
    cv::imencode(".jpg", out, jpeg, {cv::IMWRITE_JPEG_QUALITY, rate.quality});
    if (jpeg.size() > kMaxFrameBytes) {
      ++too_big_;
      RCLCPP_WARN_THROTTLE(get_logger(), *get_clock(), 5000,
        "frame %zu KB > 512 KB (dropped): lower jpeg_quality or max_width", jpeg.size() / 1024);
      return;
    }
    v1::VideoFrame f;
    f.set_frame_id(++frame_id_);
    f.set_capture_time_us(capture_us);
    const auto done = static_cast<std::uint64_t>(steady_us());
    f.set_encode_done_offset_us(static_cast<std::uint32_t>(done > capture_us ? done - capture_us : 0));
    f.set_width(static_cast<std::uint32_t>(w));
    f.set_height(static_cast<std::uint32_t>(h));
    f.set_codec(v1::VideoFrame::JPEG);
    f.set_data(jpeg.data(), jpeg.size());
    {
      std::lock_guard<std::mutex> lock(rate_mutex_);
      f.set_level(static_cast<std::uint32_t>(rate_.level()));
      rate_.on_sent(f.frame_id(), steady_us());
    }
    pub_->put(zenoh::Bytes(f.SerializeAsString()));
    last_sent_us_ = now;
    ++frames_out_;
    bytes_out_ += jpeg.size();
  }

  void report() {
    if (frames_out_ == 0 && images_in_ == 0) {
      return;
    }
    std::string adapt;
    if (adaptive_) {
      std::lock_guard<std::mutex> lock(rate_mutex_);
      const auto d = rate_.last_delay_us();
      adapt = ", level " + std::to_string(rate_.level()) +
        (d ? ", delay " + std::to_string(*d / 1000) + " ms" : std::string(", no feedback"));
    }
    RCLCPP_INFO(get_logger(), "last 5 s: %llu images in, %llu frames out (%.1f fps, %.1f KB avg)%s%s",
      static_cast<unsigned long long>(images_in_), static_cast<unsigned long long>(frames_out_),
      static_cast<double>(frames_out_) / 5.0,
      frames_out_ ? static_cast<double>(bytes_out_) / static_cast<double>(frames_out_) / 1024.0 : 0.0,
      adapt.c_str(), too_big_ ? (", " + std::to_string(too_big_) + " too big").c_str() : "");
    images_in_ = frames_out_ = bytes_out_ = too_big_ = 0;
  }

  std::string image_topic_;
  int max_width_ = 480;
  double fps_ = 30.0;
  int quality_ = 70;
  bool adaptive_ = true;

  std::mutex rate_mutex_;  // rate_: feedback arrives on a Zenoh thread
  RateController rate_;
  bool was_watching_ = false;
  int logged_level_ = 0;

  std::mutex mutex_;
  sensor_msgs::msg::Image::ConstSharedPtr latest_;
  std::int64_t latest_arrival_us_ = 0;
  std::int64_t last_sent_us_ = 0;
  std::uint32_t frame_id_ = 0;
  std::uint64_t images_in_ = 0, frames_out_ = 0, bytes_out_ = 0, too_big_ = 0;
  std::atomic<bool> wanted_{false};

  rclcpp::Subscription<sensor_msgs::msg::Image>::SharedPtr sub_;
  rclcpp::TimerBase::SharedPtr timer_;
  rclcpp::TimerBase::SharedPtr stats_timer_;
  // Last, so they go first on destruction.
  std::optional<zenoh::Session> session_;
  std::optional<zenoh::Publisher> pub_;
  std::optional<zenoh::MatchingListener<void>> matching_;
  std::optional<zenoh::Subscriber<void>> feedback_sub_;
};

}  // namespace deck_video

RCLCPP_COMPONENTS_REGISTER_NODE(deck_video::VideoPublisher)
