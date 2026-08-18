/* Thin wrappers over the libvpx C *macros* that bindgen cannot emit.
 *
 * `vpx_codec_enc_init` / `vpx_codec_dec_init` are macros that inject the
 * compile-time ABI version, and `vpx_codec_control` is a variadic macro. We
 * re-expose them here as plain `extern "C"` functions so Rust can call them
 * without reimplementing the ABI-version constants or variadic dispatch. */
#include <vpx/vpx_codec.h>
#include <vpx/vpx_encoder.h>
#include <vpx/vpx_decoder.h>
#include <vpx/vp8cx.h>
#include <vpx/vp8dx.h>

vpx_codec_err_t mezon_vpx_enc_init(vpx_codec_ctx_t *ctx,
                                   vpx_codec_iface_t *iface,
                                   const vpx_codec_enc_cfg_t *cfg,
                                   vpx_codec_flags_t flags) {
  return vpx_codec_enc_init(ctx, iface, cfg, flags);
}

vpx_codec_err_t mezon_vpx_dec_init(vpx_codec_ctx_t *ctx,
                                   vpx_codec_iface_t *iface,
                                   const vpx_codec_dec_cfg_t *cfg,
                                   vpx_codec_flags_t flags) {
  return vpx_codec_dec_init(ctx, iface, cfg, flags);
}

/* Wraps the variadic `vpx_codec_control_` for the many controls that take a
 * single `int` argument (VP8E_SET_CPUUSED, VP9E_SET_PROFILE, VP9E_SET_SVC, ...). */
vpx_codec_err_t mezon_vpx_control_int(vpx_codec_ctx_t *ctx, int ctrl_id,
                                      int val) {
  return vpx_codec_control_(ctx, ctrl_id, val);
}

/* VP9E_SET_SVC_PARAMETERS takes a pointer to a vpx_svc_extra_cfg_t. */
vpx_codec_err_t mezon_vpx_set_svc_params(vpx_codec_ctx_t *ctx,
                                         const vpx_svc_extra_cfg_t *params) {
  return vpx_codec_control_(ctx, VP9E_SET_SVC_PARAMETERS,
                            (vpx_svc_extra_cfg_t *)params);
}

/* VP9E_GET_SVC_LAYER_ID reports the spatial/temporal id of the last packet. */
vpx_codec_err_t mezon_vpx_get_svc_layer_id(vpx_codec_ctx_t *ctx,
                                           vpx_svc_layer_id_t *out) {
  return vpx_codec_control_(ctx, VP9E_GET_SVC_LAYER_ID, out);
}

/* Registers a per-spatial-layer output callback (VP9 SVC). libvpx copies the
 * pair internally, so a stack value is fine. With this registered, each spatial
 * layer of a superframe is delivered to `cb` during `vpx_codec_encode`. */
vpx_codec_err_t mezon_vpx_register_cx_callback(
    vpx_codec_ctx_t *ctx,
    void (*cb)(vpx_codec_cx_pkt_t *pkt, void *user), void *user) {
  vpx_codec_priv_output_cx_pkt_cb_pair_t pair;
  pair.output_cx_pkt = cb;
  pair.user_priv = user;
  return vpx_codec_control_(ctx, VP9E_REGISTER_CX_CALLBACK, &pair);
}
