#pragma once

#include <memory>
#include <mutex>

#include "api/data_channel_interface.h"
#include "mezon_rtc/webrtc.h"
#include "rtc_base/synchronization/mutex.h"
#include "rust/cxx.h"

namespace mezon_ffi {
class DataChannel;
}
#include "webrtc-sys/src/data_channel.rs.h"

namespace mezon_ffi {

class NativeDataChannelObserver;

webrtc::DataChannelInit to_native_data_channel_init(DataChannelInit init);

class DataChannel {
 public:
  explicit DataChannel(
      std::shared_ptr<RtcRuntime> rtc_runtime,
      webrtc::scoped_refptr<webrtc::DataChannelInterface> data_channel);
  ~DataChannel();

  void register_observer(rust::Box<DataChannelObserverWrapper> observer) const;
  void unregister_observer() const;
  bool send(const DataBuffer& buffer) const;
  int id() const;
  rust::String label() const;
  DataState state() const;
  void close() const;
  uint64_t buffered_amount() const;

 private:
  mutable webrtc::Mutex mutex_;
  std::shared_ptr<RtcRuntime> rtc_runtime_;
  webrtc::scoped_refptr<webrtc::DataChannelInterface> data_channel_;
  mutable std::unique_ptr<NativeDataChannelObserver> observer_;
};

static std::shared_ptr<DataChannel> _shared_data_channel() {
  return nullptr;  // Ignore
}

class NativeDataChannelObserver : public webrtc::DataChannelObserver {
 public:
  NativeDataChannelObserver(rust::Box<DataChannelObserverWrapper> observer,
                            const DataChannel* dc);

  void OnStateChange() override;
  void OnMessage(const webrtc::DataBuffer& buffer) override;
  void OnBufferedAmountChange(uint64_t sent_data_size) override;

 private:
  rust::Box<DataChannelObserverWrapper> observer_;
  const DataChannel* dc_;
};

}
