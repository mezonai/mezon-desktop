# Local patches to nokhwa-bindings-macos 0.2.4

Source: crates.io nokhwa-bindings-macos 0.2.4, Apache-2.0.

- Pass an NSError output pointer to lockForConfiguration and report its description.
- Compare the Objective-C BOOL with NO on both Intel and Apple Silicon.
- Record successful locks so unlock actually releases the device.
- Release configuration locks on both successful and failed format selection.

- Match resolution, pixel format, and frame-rate range on the same native format.
- Accept minimum and interior FPS values, preserving native fractional endpoint durations.
- Apply the selected frame duration instead of always forcing the maximum FPS.

Run the dependency-free FPS regression tests with:

```sh
rustc --edition 2021 --test crates/vendor/nokhwa-bindings-macos/src/frame_rate.rs -o /tmp/mezon-camera-frame-rate-tests
/tmp/mezon-camera-frame-rate-tests
```

The workspace patches this crate locally without changing its version or dependencies.
