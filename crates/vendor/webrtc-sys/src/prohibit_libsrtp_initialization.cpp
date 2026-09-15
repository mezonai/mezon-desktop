#include "pc/srtp_session.h"

namespace mezon_ffi {
void ProhibitLibsrtpInitialization() {
    webrtc::ProhibitLibsrtpInitialization();
}
}