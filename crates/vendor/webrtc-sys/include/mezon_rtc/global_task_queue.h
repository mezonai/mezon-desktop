#pragma once

#include "api/task_queue/task_queue_factory.h"

namespace mezon_ffi {

webrtc::TaskQueueFactory* GetGlobalTaskQueueFactory();

}
