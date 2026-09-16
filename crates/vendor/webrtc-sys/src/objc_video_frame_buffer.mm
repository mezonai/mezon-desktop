#include "mezon_rtc/video_frame_buffer.h"

#import <CoreVideo/CoreVideo.h>
#import <sdk/objc/components/video_frame_buffer/RTCCVPixelBuffer.h>
#include "sdk/objc/native/api/video_frame_buffer.h"

namespace mezon_ffi {

std::unique_ptr<VideoFrameBuffer> new_native_buffer_from_platform_image_buffer(
    CVPixelBufferRef pixelBuffer
) {
    RTC_OBJC_TYPE(RTCCVPixelBuffer) *buffer = [[RTC_OBJC_TYPE(RTCCVPixelBuffer) alloc] initWithPixelBuffer:pixelBuffer];
    webrtc::scoped_refptr<webrtc::VideoFrameBuffer> frame_buffer = webrtc::ObjCToNativeVideoFrameBuffer(buffer);
    [buffer release];
    CVPixelBufferRelease(pixelBuffer);
    return std::make_unique<VideoFrameBuffer>(frame_buffer);
}

CVPixelBufferRef native_buffer_to_platform_image_buffer(
    const std::unique_ptr<VideoFrameBuffer> &buffer
) {
    id<RTC_OBJC_TYPE(RTCVideoFrameBuffer)> rtc_pixel_buffer = webrtc::NativeToObjCVideoFrameBuffer(buffer->get());

    if ([rtc_pixel_buffer isKindOfClass:[RTC_OBJC_TYPE(RTCCVPixelBuffer) class]]) {
        RTC_OBJC_TYPE(RTCCVPixelBuffer) *cv_pixel_buffer = (RTC_OBJC_TYPE(RTCCVPixelBuffer) *)rtc_pixel_buffer;
        return [cv_pixel_buffer pixelBuffer];
    } else {
        return nullptr;
    }
}

}
