use crate::webview::ChannelAppWebView;

#[cfg(not(target_os = "windows"))]
pub fn reload(webview: &ChannelAppWebView) {
    if let Err(error) = webview.evaluate_script("location.reload()") {
        tracing::warn!("channel app webview reload failed: {error:#}");
    }
}

#[cfg(target_os = "windows")]
pub fn reload(webview: &ChannelAppWebView) {
    use wry::WebViewExtWindows;

    let controller = webview.controller();
    let core = match unsafe { controller.CoreWebView2() } {
        Ok(core) => core,
        Err(error) => {
            tracing::warn!("channel app webview reload failed: {error:#}");
            return;
        }
    };
    if let Err(error) = unsafe { core.Reload() } {
        tracing::warn!("channel app webview reload failed: {error:#}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn go_back(webview: &ChannelAppWebView) {
    if let Err(error) = webview.evaluate_script("history.back()") {
        tracing::warn!("channel app webview back failed: {error:#}");
    }
}

#[cfg(target_os = "windows")]
pub fn go_back(webview: &ChannelAppWebView) {
    use wry::WebViewExtWindows;

    let controller = webview.controller();
    let core = match unsafe { controller.CoreWebView2() } {
        Ok(core) => core,
        Err(error) => {
            tracing::warn!("channel app webview back failed: {error:#}");
            return;
        }
    };
    if let Err(error) = unsafe { core.GoBack() } {
        tracing::warn!("channel app webview back failed: {error:#}");
    }
}

#[cfg(not(target_os = "windows"))]
pub fn go_forward(webview: &ChannelAppWebView) {
    if let Err(error) = webview.evaluate_script("history.forward()") {
        tracing::warn!("channel app webview forward failed: {error:#}");
    }
}

#[cfg(target_os = "windows")]
pub fn go_forward(webview: &ChannelAppWebView) {
    use wry::WebViewExtWindows;

    let controller = webview.controller();
    let core = match unsafe { controller.CoreWebView2() } {
        Ok(core) => core,
        Err(error) => {
            tracing::warn!("channel app webview forward failed: {error:#}");
            return;
        }
    };
    if let Err(error) = unsafe { core.GoForward() } {
        tracing::warn!("channel app webview forward failed: {error:#}");
    }
}

pub fn current_url(webview: Option<&ChannelAppWebView>, fallback: &str) -> String {
    webview
        .and_then(|webview| webview.url().ok())
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}
