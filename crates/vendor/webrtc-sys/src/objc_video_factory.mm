#include "mezon_rtc/objc_video_factory.h"

#import <sdk/objc/components/video_codec/RTCDefaultVideoDecoderFactory.h>
#import <sdk/objc/components/video_codec/RTCDefaultVideoEncoderFactory.h>
#import <sdk/objc/components/video_codec/RTCVideoEncoderFactorySimulcast.h>
#include "sdk/objc/native/api/video_decoder_factory.h"
#include "sdk/objc/native/api/video_encoder_factory.h"

namespace mezon_ffi {

std::unique_ptr<webrtc::VideoEncoderFactory> CreateObjCVideoEncoderFactory() {
  RTC_OBJC_TYPE(RTCDefaultVideoEncoderFactory)* encoderFactory = [[RTC_OBJC_TYPE(RTCDefaultVideoEncoderFactory) alloc] init];
  RTC_OBJC_TYPE(RTCVideoEncoderFactorySimulcast)* simulcastFactory =
      [[RTC_OBJC_TYPE(RTCVideoEncoderFactorySimulcast) alloc] initWithPrimary:encoderFactory fallback:encoderFactory];
  return webrtc::ObjCToNativeVideoEncoderFactory(simulcastFactory);
}

std::unique_ptr<webrtc::VideoDecoderFactory> CreateObjCVideoDecoderFactory() {
  return webrtc::ObjCToNativeVideoDecoderFactory([[RTC_OBJC_TYPE(RTCDefaultVideoDecoderFactory) alloc] init]);
}

}
