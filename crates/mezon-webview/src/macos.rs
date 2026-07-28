use anyhow::Context;
use raw_window_handle::HasWindowHandle;
use wry::WebViewBuilder;

use crate::webview::ChannelAppWebView;

pub fn create(
    parent: &impl HasWindowHandle,
    builder: WebViewBuilder<'_>,
) -> anyhow::Result<ChannelAppWebView> {
    builder
        .build_as_child(parent)
        .map(ChannelAppWebView::new)
        .context("Failed to create channel app webview")
}
