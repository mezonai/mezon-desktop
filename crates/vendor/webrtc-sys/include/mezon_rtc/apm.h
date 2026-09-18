#pragma once

#include <memory>

#include "api/scoped_refptr.h"
#include "api/video_codecs/video_decoder_factory.h"
#include "api/video_codecs/video_encoder_factory.h"
#include "modules/audio_processing/aec3/echo_canceller3.h"
#include "modules/audio_processing/audio_buffer.h"

namespace mezon_ffi {

struct AudioProcessingConfig {
  bool echo_canceller_enabled;
  bool gain_controller_enabled;
  bool high_pass_filter_enabled;
  bool noise_suppression_enabled;

  webrtc::AudioProcessing::Config ToWebrtcConfig() const {
    webrtc::AudioProcessing::Config config;
    config.echo_canceller.enabled = echo_canceller_enabled;
    config.gain_controller2.enabled = gain_controller_enabled;
    config.gain_controller2.adaptive_digital.enabled = gain_controller_enabled;
    config.high_pass_filter.enabled = high_pass_filter_enabled;
    config.noise_suppression.enabled = noise_suppression_enabled;
    return config;
  }
};

class AudioProcessingModule {
 public:
  AudioProcessingModule(const AudioProcessingConfig& config);

  int process_stream(const int16_t* src,
                     size_t src_len,
                     int16_t* dst,
                     size_t dst_len,
                     int sample_rate,
                     int num_channels);

  int process_reverse_stream(const int16_t* src,
                             size_t src_len,
                             int16_t* dst,
                             size_t dst_len,
                             int sample_rate,
                             int num_channels);

  int set_stream_delay_ms(int delay_ms);

 private:
  webrtc::scoped_refptr<webrtc::AudioProcessing> apm_;
};

std::unique_ptr<AudioProcessingModule> create_apm(
    bool echo_canceller_enabled,
    bool gain_controller_enabled,
    bool high_pass_filter_enabled,
    bool noise_suppression_enabled);

}
