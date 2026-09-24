#pragma once

#include <memory>

#include "api/audio/audio_frame.h"
#include "api/data_channel_interface.h"
#include "common_audio/resampler/include/push_resampler.h"
#include "mezon_rtc/webrtc.h"
#include "rust/cxx.h"

namespace mezon_ffi {

class AudioResampler {
 public:
  size_t remix_and_resample(const int16_t* src,
                            size_t samples_per_channel,
                            size_t num_channels,
                            int sample_rate_hz,
                            size_t dest_num_channels,
                            int dest_sample_rate_hz);

  const int16_t* data() const;

 private:
  webrtc::AudioFrame frame_;
  webrtc::PushResampler<int16_t> resampler_;
};

std::unique_ptr<AudioResampler> create_audio_resampler();

}
