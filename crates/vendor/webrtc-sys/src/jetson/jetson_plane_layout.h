#ifndef MEZON_RTC_JETSON_PLANE_LAYOUT_H_
#define MEZON_RTC_JETSON_PLANE_LAYOUT_H_

namespace mezon_rtc {

/// Plain-data view of everything known about a destination plane's memory
/// layout at copy time.
///
/// These values are sourced from (sometimes unreliable) Jetson MMAPI /
/// `NvBufSurface` metadata, but the struct is intentionally free of any
/// hardware types so that the resolution logic below is pure and unit-testable.
struct PlaneLayoutHints {
  /// `NvBuffer::NvBufferPlane::fmt.stride`. Often `0`/unset in MMAP mode.
  int fmt_stride = 0;
  /// Pitch reported by `NvBufSurfaceFromFd` for this plane's fd, or `0` when the
  /// probe is unavailable/unsupported on the running JetPack version.
  int probed_pitch = 0;
  /// Byte length of the mapped plane, or `0` when unknown.
  int plane_length = 0;
  /// `NvBuffer::NvBufferPlane::fmt.height`. May be misleading (e.g. a UV plane
  /// reporting full luma height), so it is only trusted as a divisor when it
  /// yields a stride that meets `min_stride`.
  int fmt_height = 0;
};

/// Resolves the destination stride (bytes per row) to use when writing a plane.
///
/// `expected_height` is the number of rows that will be written, `min_stride`
/// the hard lower bound (typically the plane width in bytes), and `fallback`
/// the value to use when no reliable source is available.
///
/// Preference order, each accepted only when it is at least `min_stride`:
/// [`PlaneLayoutHints::fmt_stride`] -> [`PlaneLayoutHints::probed_pitch`] ->
/// stride derived from [`PlaneLayoutHints::plane_length`] -> `fallback`. The
/// result is clamped to `min_stride` and, when derivable, to
/// `plane_length / expected_height` so the caller never under- or over-strides
/// the mapped allocation.
int ResolvePlaneStride(const PlaneLayoutHints& hints,
                       int expected_height,
                       int min_stride,
                       int fallback);

/// Resolves the number of rows the destination plane can hold.
///
/// Uses `probed_height` only when `have_probe` is set and the probed value is at
/// least `expected_height`; otherwise returns `expected_height`.
int ResolvePlaneHeight(bool have_probe, int probed_height, int expected_height);

}

#endif
