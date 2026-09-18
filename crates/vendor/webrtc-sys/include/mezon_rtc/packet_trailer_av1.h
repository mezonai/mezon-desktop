#pragma once

#include <cstdint>
#include <optional>
#include <vector>

#include "api/array_view.h"
#include "api/frame_transformer_interface.h"
#include "mezon_rtc/packet_trailer.h"

namespace mezon_ffi {
namespace av1 {

/// Returns true if the frame's MIME type identifies it as AV1.
bool IsAv1Frame(const webrtc::TransformableFrameInterface& frame);

std::vector<uint8_t> InsertTrailerObu(
    webrtc::ArrayView<const uint8_t> data,
    webrtc::ArrayView<const uint8_t> trailer);

std::optional<PacketTrailerMetadata> ExtractTrailer(
    webrtc::ArrayView<const uint8_t> data,
    std::vector<uint8_t>& out_data);

}  // namespace av1
}
