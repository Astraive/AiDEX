//! Keeps the main webview on the app's own pages.
//!
//! The window sets `dragDropEnabled: false` so HTML5 drops deliver files to the page. WebView2 then
//! handles external drops itself, and a file the page does not cancel would replace the app with
//! that file (an HTML file would run script in the main window). The page cancels every file drop
//! (`useFileDropGuard` at the app root); this refuses any navigation that slips past it. External
//! links never navigate the window: they open through the daemon's `web.open`.

use tauri::{plugin::TauriPlugin, Manager, Runtime, Url};

/// Whether the main webview may navigate to `url`: the app's own origin (`tauri://localhost`, or
/// `http(s)://tauri.localhost` on Windows), the dev server's origin in development (`dev_url`), a
/// `blob:` URL of one of those (downloads), or `about:blank`.
pub(crate) fn allowed_navigation(url: &Url, dev_url: Option<&Url>) -> bool {
    match url.scheme() {
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || dev_url.is_some_and(|dev| same_origin(url, dev))
        }
        "blob" => Url::parse(url.path())
            .is_ok_and(|inner| inner.scheme() != "blob" && allowed_navigation(&inner, dev_url)),
        "about" => url.path() == "blank",
        _ => false,
    }
}

fn same_origin(a: &Url, b: &Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

/// A plugin (no commands, no permissions) whose navigation hook applies [`allowed_navigation`].
pub(crate) fn plugin<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new("aidex-navigation")
        .on_navigation(|webview, url| {
            let dev_url = if tauri::is_dev() {
                webview.config().build.dev_url.clone()
            } else {
                None
            };
            allowed_navigation(url, dev_url.as_ref())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::allowed_navigation;
    use tauri::Url;

    fn allowed(url: &str, dev: Option<&str>) -> bool {
        let dev = dev.map(|dev| Url::parse(dev).unwrap());
        allowed_navigation(&Url::parse(url).unwrap(), dev.as_ref())
    }

    #[test]
    fn the_app_origin_is_allowed() {
        assert!(allowed("tauri://localhost/index.html", None));
        assert!(allowed("http://tauri.localhost/", None));
        assert!(allowed("https://tauri.localhost/chat", None));
        assert!(allowed("about:blank", None));
        assert!(allowed("blob:http://tauri.localhost/6d1c2f0e", None));
    }

    #[test]
    fn dropped_files_and_other_sites_are_refused() {
        assert!(!allowed("file:///C:/Users/me/Downloads/page.html", None));
        assert!(!allowed("file:///home/me/notes.pdf", None));
        assert!(!allowed("https://example.com/", None));
        assert!(!allowed("http://localhost:3311/", None));
        assert!(!allowed("tauri://example/", None));
        assert!(!allowed("data:text/html,<script>alert(1)</script>", None));
        assert!(!allowed("javascript:alert(1)", None));
        assert!(!allowed("about:srcdoc", None));
        assert!(!allowed("blob:https://example.com/6d1c2f0e", None));
        assert!(!allowed("blob:blob:http://tauri.localhost/x", None));
    }

    #[test]
    fn only_the_dev_server_origin_is_added_in_development() {
        let dev = Some("http://localhost:3311");
        assert!(allowed("http://localhost:3311/", dev));
        assert!(allowed("http://localhost:3311/_next/static/x.js", dev));
        assert!(!allowed("http://localhost:3312/", dev));
        assert!(!allowed("https://localhost:3311/", dev));
        assert!(!allowed("http://127.0.0.1:3311/", dev));
    }
}
