import SwiftUI
import TomiDesign
import TomiKit
import UIKit
import WebKit

/// Received HTML in a sandboxed web view (DEV.md invariant 6, Android `SandboxedHtmlView`): sanitised by
/// `HtmlSanitizer`, wrapped in a document whose CSP forbids every load but inline `data:` images, no JavaScript of the
/// page, no persistent storage, no navigation (a tapped http / https / mailto / tel link opens outside, nothing else
/// does anything). Reports its content height so the card can cap it and offer "Lire la suite".
struct SandboxedHTMLView: UIViewRepresentable {
    let html: String
    /// Detail screen: the page may scroll sideways (wide tables). Card: never, so a sideways drag stays the card's.
    var horizontalScroll: Bool
    @Binding var contentHeight: CGFloat
    @Environment(\.colorScheme) private var colorScheme

    func makeCoordinator() -> Coordinator { Coordinator(height: $contentHeight) }

    func makeUIView(context: Context) -> WKWebView {
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.defaultWebpagePreferences.allowsContentJavaScript = false
        configuration.preferences.javaScriptCanOpenWindowsAutomatically = false
        configuration.dataDetectorTypes = []
        configuration.allowsInlineMediaPlayback = false
        configuration.mediaTypesRequiringUserActionForPlayback = .all
        let view = WKWebView(frame: .zero, configuration: configuration)
        view.navigationDelegate = context.coordinator
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.backgroundColor = .clear
        view.scrollView.isScrollEnabled = horizontalScroll
        view.scrollView.bounces = false
        view.allowsLinkPreview = false
        view.isInspectable = false
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        let document = HtmlSanitizer.toDocument(
            html,
            textColorCss: Self.css(TomiColor.text, colorScheme),
            linkColorCss: Self.css(TomiColor.brand, colorScheme),
            horizontalScroll: horizontalScroll
        )
        guard context.coordinator.loaded != document else { return }
        context.coordinator.loaded = document
        // No base URL: relative references resolve to nothing.
        view.loadHTMLString(document, baseURL: nil)
    }

    private static func css(_ color: Color, _ scheme: ColorScheme) -> String {
        let resolved = UIColor(color).resolvedColor(with: UITraitCollection(userInterfaceStyle: scheme == .dark ? .dark : .light))
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        resolved.getRed(&r, green: &g, blue: &b, alpha: &a)
        return String(format: "#%02X%02X%02X", Int(r * 255), Int(g * 255), Int(b * 255))
    }

    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: String?
        private let height: Binding<CGFloat>

        init(height: Binding<CGFloat>) {
            self.height = height
        }

        func webView(
            _ webView: WKWebView,
            decidePolicyFor action: WKNavigationAction,
            decisionHandler: @escaping @MainActor (WKNavigationActionPolicy) -> Void
        ) {
            // Only the document we load ourselves (about:blank from loadHTMLString).
            if action.navigationType == .other, action.request.url?.scheme == "about" {
                decisionHandler(.allow)
                return
            }
            if action.navigationType == .linkActivated, let url = action.request.url, SafeLinks.isOpenable(url) {
                UIApplication.shared.open(url)
            }
            decisionHandler(.cancel)
        }

        func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
            // Script of the app (the page's own scripts stay disabled): the height of the content.
            webView.evaluateJavaScript("document.documentElement.scrollHeight") { [height] value, _ in
                if let value = value as? CGFloat { height.wrappedValue = value }
            }
        }
    }
}
