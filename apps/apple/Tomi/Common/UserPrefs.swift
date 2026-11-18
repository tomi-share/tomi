import Foundation
import TomiKit

/// Preferences of this device (Android `UserPrefs`, same keys). In the App Group suite once the
/// share extension exists (PR-8).
final class UserPrefs {
    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    /// Last text display mode chosen ("Aperçu" / "Markdown"); the default for every card.
    var preferredRenderMode: RenderMode? {
        get {
            switch defaults.string(forKey: "render_mode") {
            case "Preview": .preview
            case "Source": .source
            default: nil
            }
        }
        set {
            defaults.set(newValue.map { $0 == .preview ? "Preview" : "Source" }, forKey: "render_mode")
        }
    }

    var codeWrapEnabled: Bool {
        get { defaults.bool(forKey: "code_wrap") }
        set { defaults.set(newValue, forKey: "code_wrap") }
    }

    var onboardingDone: Bool {
        get { defaults.bool(forKey: "onboarding_done") }
        set { defaults.set(newValue, forKey: "onboarding_done") }
    }
}
