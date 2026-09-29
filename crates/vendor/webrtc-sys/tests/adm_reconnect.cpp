// Standalone regression test: compile with src/adm_proxy.cpp and
// src/synthetic_audio_device.cpp, linking the same prebuilt WebRTC as the app.
// Uses synthetic playout only; no microphone capture or speaker output.
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <mutex>
#include <thread>

#include "api/environment/environment_factory.h"
#include "api/make_ref_counted.h"
#include "mezon_rtc/adm_proxy.h"
#include "rtc_base/thread.h"

namespace {
void Check(bool ok, const char* message) {
  if (!ok) {
    std::fprintf(stderr, "FAIL: %s\n", message);
    std::abort();
  }
}

class CountingTransport : public webrtc::AudioTransport {
 public:
  int32_t RecordedDataIsAvailable(const void*, size_t, size_t, size_t,
                                 uint32_t, uint32_t, int32_t, uint32_t,
                                 bool, uint32_t&) override {
    return 0;
  }

  int32_t NeedMorePlayData(size_t samples, size_t bytes_per_sample,
                          size_t, uint32_t, void* audio,
                          size_t& samples_out, int64_t* elapsed,
                          int64_t* ntp) override {
    std::memset(audio, 0, samples * bytes_per_sample);
    samples_out = samples;
    *elapsed = 0;
    *ntp = 0;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      ++calls_;
    }
    changed_.notify_all();
    return 0;
  }

  void PullRenderData(int, int, size_t, size_t, void*,
                      int64_t*, int64_t*) override {}

  bool WaitForProgress(int baseline) {
    std::unique_lock<std::mutex> lock(mutex_);
    return changed_.wait_for(lock, std::chrono::seconds(2),
                            [&] { return calls_ > baseline; });
  }

  int calls() const { return calls_.load(); }

 private:
  std::atomic<int> calls_{0};
  std::mutex mutex_;
  std::condition_variable changed_;
};
}  // namespace

int main() {
  auto env = webrtc::CreateEnvironment();
  auto worker = webrtc::Thread::Create();
  Check(worker->Start(), "worker starts");
  worker->BlockingCall([&] {
    // A new/reinitialized pump must tolerate a not-yet-registered transport.
    auto synthetic = webrtc::make_ref_counted<mezon_ffi::SyntheticAudioDevice>(env);
    Check(synthetic->Init() == 0, "synthetic init");
    Check(synthetic->StartPlayout() == 0, "synthetic start before callback");
    std::this_thread::sleep_for(std::chrono::milliseconds(30));
    Check(synthetic->Terminate() == 0, "synthetic terminate");
    Check(!synthetic->Playing(), "terminated synthetic is not playing");

    CountingTransport transport;
    auto adm = webrtc::make_ref_counted<mezon_ffi::AdmProxy>(env, worker.get());
    for (int epoch = 0; epoch < 4; ++epoch) {
      Check(adm->Init() == 0, "ADM init succeeds after terminate");
      Check(adm->Initialized(), "ADM is initialized after reconnect");
      Check(adm->Init() == 0, "repeated init is safe");
      Check(!adm->Playing(), "init does not resume old playout");
      const int before_start = transport.calls();
      std::this_thread::sleep_for(std::chrono::milliseconds(30));
      Check(transport.calls() == before_start, "old transport stays stopped");
      if (adm->PlayoutDevices() > 0) {
        Check(adm->SetPlayoutDevice(uint16_t{0}) == 0,
              "platform device can be selected after reconnect");
      }
      Check(adm->RegisterAudioCallback(&transport) == 0, "register callback");
      Check(adm->InitPlayout() == 0, "init playout");
      Check(adm->StartPlayout() == 0, "start playout");
      Check(transport.WaitForProgress(before_start), "decoded-audio pump resumes");
      Check(adm->Terminate() == 0, "terminate");
      Check(!adm->Initialized(), "terminate releases ADM initialization");
      Check(!adm->Playing(), "terminate stops playout");
      Check(adm->Terminate() == 0, "repeated terminate is safe");
    }
    adm->RegisterAudioCallback(nullptr);
  });
  worker->Stop();
  std::puts("PASS: ADM audio pump survives 3 reconnects; null callback is safe");
}
