#pragma once

#include <memory>

#include "api/candidate.h"

namespace mezon_ffi {
class Candidate;
}
#include "webrtc-sys/src/candidate.rs.h"

// webrtc::Candidate
namespace mezon_ffi {

class Candidate {
 public:
  explicit Candidate(const webrtc::Candidate& candidate);

 private:
  webrtc::Candidate candidate_;
};

static std::shared_ptr<Candidate> _shared_candidate() {
  return nullptr;
}

}
