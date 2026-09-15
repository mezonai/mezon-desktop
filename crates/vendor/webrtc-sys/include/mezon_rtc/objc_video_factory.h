#pragma once

#include <memory>

#include "api/video_codecs/video_decoder_factory.h"
#include "api/video_codecs/video_encoder_factory.h"

namespace mezon_ffi {

std::unique_ptr<webrtc::VideoEncoderFactory> CreateObjCVideoEncoderFactory();
std::unique_ptr<webrtc::VideoDecoderFactory> CreateObjCVideoDecoderFactory();

}
