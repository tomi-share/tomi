#if os(iOS)
import Foundation
import UIKit
import UserNotifications

/// The push doorbell of the iPhone (`api-v1.md` §6.4). iOS stops the app when it leaves the screen: without a push
/// nothing arrives until the app is opened again. The relay, which knows nothing of an item, asks Apple for a
/// notification with fixed words ("Tomi", "New item"); iOS shows it at once and, when it can, wakes the app,
/// which fetches the item and replaces the fixed words by its own notification (kind, sender, preview; never
/// anything of a hidden item).
///
/// The Mac app needs none of this: it stays in the menu bar with its doorbell open.
@MainActor
enum PushRegistration {
    /// The identifier of the generic notification (`apns-collapse-id`): one at most on the device.
    static let genericId = "tomi"

    /// The token Apple gave for this app on this device, in hexadecimal. Nil until iOS answered (no network yet,
    /// a simulator that cannot register).
    private(set) static var token: String?

    /// Asks iOS for the token (answered in the app delegate). Needs no permission: the permission to show
    /// notifications is asked apart, and without it a push still wakes the app.
    static func start() {
        UIApplication.shared.registerForRemoteNotifications()
    }

    static func received(_ deviceToken: Data) {
        token = deviceToken.map { String(format: "%02x", $0) }.joined()
        AppContainer.current?.registerPush()
    }

    /// Which endpoint of Apple the token is for: the one the app was signed for. A build installed from Xcode
    /// carries its provisioning profile, which says `development`; an App Store or TestFlight build has none and is
    /// `production`. A token given to the wrong endpoint is refused by Apple.
    static let provider: String = {
        #if targetEnvironment(simulator)
        // A simulator has no profile, and its tokens are development ones.
        return "apns-sandbox"
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: url) else { return "apns" }
        return environment(inProfile: data) == "development" ? "apns-sandbox" : "apns"
        #endif
    }()

    /// The `aps-environment` of a provisioning profile (a signed envelope around a property list in clear).
    nonisolated static func environment(inProfile data: Data) -> String? {
        let text = String(decoding: data, as: UTF8.self)
        guard let key = text.range(of: "<key>aps-environment</key>"),
              let open = text.range(of: "<string>", range: key.upperBound ..< text.endIndex),
              let close = text.range(of: "</string>", range: open.upperBound ..< text.endIndex) else { return nil }
        // Right after the key: not the value of a later one.
        guard text[key.upperBound ..< open.lowerBound].allSatisfy(\.isWhitespace) else { return nil }
        return String(text[open.upperBound ..< close.lowerBound])
    }

    /// A push woke the app: it fetches what waits and shows its own notifications (without their sound when the
    /// generic one just rang), then takes the generic notification down. Kept when nothing could be fetched: it is
    /// then all the person has.
    static func woken(rang: Bool) async -> UIBackgroundFetchResult {
        guard let container = AppContainer.current else { return .noData }
        let fetched = await container.runInBackground(sendOnly: false, push: rang ? .rang : .silent)
        if fetched { removeGeneric() }
        return fetched ? .newData : .failed
    }

    /// The generic notification has nothing more to say: the app showed its own, or is now in front of the person
    /// (iOS does not wake an app that was swiped away, and may not wake it at all).
    static func removeGeneric() {
        UNUserNotificationCenter.current().removeDeliveredNotifications(withIdentifiers: [genericId])
    }
}
#endif
