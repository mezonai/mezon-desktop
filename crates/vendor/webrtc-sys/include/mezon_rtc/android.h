#pragma once

#include <jni.h>

#include <cstdint>
#include <memory>

#include "api/video_codecs/video_decoder_factory.h"
#include "api/video_codecs/video_encoder_factory.h"

namespace mezon_ffi {
typedef JavaVM JavaVM;
}
#include "webrtc-sys/src/android.rs.h"

namespace mezon_ffi {

/// Initialize Android WebRTC with the JVM.
/// This is called automatically by init_android_context(), so you only need to
/// call this directly in JNI_OnLoad or if you don't have an Android Context.
/// This function is idempotent - safe to call multiple times.
///
/// @param jvm The JavaVM pointer
void init_android(JavaVM* jvm);

/// Initialize Android WebRTC with the application context.
/// This is the main initialization function - it calls init_android() internally
/// and then initializes ContextUtils for PlatformAudio support.
/// This function is idempotent - safe to call multiple times.
///
/// @param jvm The JavaVM pointer
/// @param context The Android application context (jobject cast to uintptr_t)
/// @return true if context initialization was successful, false otherwise.
///         Note: JVM init (init_android) always happens regardless of return value.
bool init_android_context(JavaVM* jvm, uintptr_t context);

std::unique_ptr<webrtc::VideoEncoderFactory> CreateAndroidVideoEncoderFactory();
std::unique_ptr<webrtc::VideoDecoderFactory> CreateAndroidVideoDecoderFactory();

}
