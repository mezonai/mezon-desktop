#include "mezon_rtc/candidate.h"

namespace mezon_ffi {
Candidate::Candidate(const webrtc::Candidate& candidate)
    : candidate_(candidate) {}
}
