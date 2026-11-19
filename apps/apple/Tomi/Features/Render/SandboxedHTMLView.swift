import SwiftUI
import TomiDesign
import TomiKit
import UIKit
import WebKit

/// Received HTML in a sandboxed web view (DEV.md invariant 6, Android `SandboxedHtmlView`): sanitised by
/// `HtmlSanitizer`, wrapped in a document whose CSP forbids every load but inline `data:` images, no JavaScript of the
/// page, no persistent storage, no navigation (a tapped http / https / mailto / tel link opens outside, nothing else
/// does anything). A content rule list (`HTMLContentRules`) blocks every network scheme as a further layer, independent
/// of the sanitiser and the CSP. Reports its content height so the card can cap it and offer "Lire la suite".
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
        // Our own controller, kept by the coordinator: the rule list is added to it, now when it is compiled already, else
        // before the first load (`Coordinator.load`).
        let userContent = WKUserContentController()
        configuration.userContentController = userContent
        context.coordinator.userContent = userContent
        if let rules = HTMLContentRules.compiled {
            userContent.add(rules)
            context.coordinator.rulesInstalled = true
        }
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
        context.coordinator.load(document, in: view)
    }

    private static func css(_ color: Color, _ scheme: ColorScheme) -> String {
        let resolved = UIColor(color).resolvedColor(with: UITraitCollection(userInterfaceStyle: scheme == .dark ? .dark : .light))
        var r: CGFloat = 0, g: CGFloat = 0, b: CGFloat = 0, a: CGFloat = 0
        resolved.getRed(&r, green: &g, blue: &b, alpha: &a)
        return String(format: "#%02X%02X%02X", Int(r * 255), Int(g * 255), Int(b * 255))
    }

    @MainActor
    final class Coordinator: NSObject, WKNavigationDelegate {
        var loaded: String?
        var userContent: WKUserContentController?
        var rulesInstalled = false
        private var loading: Task<Void, Never>?
        private let height: Binding<CGFloat>

        init(height: Binding<CGFloat>) {
            self.height = height
        }

        /// Loads `document` once the content rule list is in place (the first compilation takes a moment, then it is
        /// kept). When it cannot be compiled the page still loads: the sanitiser, the CSP and the navigation policy remain.
        func load(_ document: String, in view: WKWebView) {
            loading?.cancel()
            if rulesInstalled || HTMLContentRules.finished {
                installRules()
                // No base URL: relative references resolve to nothing.
                view.loadHTMLString(document, baseURL: nil)
                return
            }
            loading = Task { @MainActor [weak self, weak view] in
                await HTMLContentRules.prepare()
                guard !Task.isCancelled, let self, let view, self.loaded == document else { return }
                self.installRules()
                view.loadHTMLString(document, baseURL: nil)
            }
        }

        private func installRules() {
            guard !rulesInstalled, let rules = HTMLContentRules.compiled else { return }
            rulesInstalled = true
            userContent?.add(rules)
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

/// A WebKit content rule list that blocks every load over the network (http, https, ws, wss, ftp) in the web view of
/// received HTML: a further layer under the sanitiser, the page's CSP and the navigation policy, so that no network
/// request can leave even if the first two were bypassed. `data:` images and the `about:` document we load ourselves
/// match no rule. One rule per scheme: the rule syntax has no alternation.
@MainActor
enum HTMLContentRules {
    static let identifier = "eu.shaffe.tomi.html-no-network"
    static let rules = """
    [
      {"trigger": {"url-filter": "^https?://"}, "action": {"type": "block"}},
      {"trigger": {"url-filter": "^wss?://"}, "action": {"type": "block"}},
      {"trigger": {"url-filter": "^ftp://"}, "action": {"type": "block"}}
    ]
    """

    /// The compiled list, nil until `prepare` has finished (and when it could not be compiled).
    private(set) static var compiled: WKContentRuleList?
    /// `prepare` has finished (the list is `compiled`, or could not be).
    private(set) static var finished = false
    private static var compilation: Task<Void, Never>?

    /// Compiles the list once (WebKit keeps it on disk); every caller waits for the same compilation.
    static func prepare() async {
        if finished { return }
        let task = compilation ?? Task { @MainActor in
            compiled = try? await WKContentRuleListStore.default().compileContentRuleList(
                forIdentifier: identifier, encodedContentRuleList: rules
            )
            finished = true
        }
        compilation = task
        await task.value
    }
}
