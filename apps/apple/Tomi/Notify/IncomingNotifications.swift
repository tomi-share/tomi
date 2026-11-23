import Foundation
import ImageIO
import TomiKit
#if canImport(UIKit)
import UIKit
#endif
import UniformTypeIdentifiers
import UserNotifications

/// Posts the notifications of received items (Android `IncomingNotifications`). What they say is
/// `IncomingNotificationContent` (never anything of a hidden item); what to post or take down is
/// `IncomingNotifierState`. The system groups them by kind (thread), as Android summarises a burst.
@MainActor
final class IncomingNotifications {
    static let categoryCopy = "eu.shaffe.tomi.incoming.copy"
    static let categoryView = "eu.shaffe.tomi.incoming.view"
    static let categoryPlain = "eu.shaffe.tomi.incoming"
    static let actionCopy = "copy"
    static let actionView = "view"
    static let itemIdKey = "itemId"
    /// `userInfo` key of the "Nouvel appareil" notification: its tap opens the devices screen.
    static let opensDevicesKey = "opensDevices"

    private let center = UNUserNotificationCenter.current()
    /// Runs for which the notifications are shown without their sound (the generic notification of a push just rang
    /// for them). A count: two pushes, or a push and a refresh of iOS, may run at once.
    var quietRuns = 0

    /// Once, at start-up: the categories and their buttons ("Copier" / "Voir"). Hidden previews show the generic title.
    func registerCategories() {
        let placeholder = L10n.text("notif_public_title")
        let copy = UNNotificationAction(identifier: Self.actionCopy, title: L10n.text("notif_action_copy"), options: [.foreground])
        let view = UNNotificationAction(identifier: Self.actionView, title: L10n.text("notif_action_view"), options: [.foreground])
        center.setNotificationCategories([
            UNNotificationCategory(identifier: Self.categoryCopy, actions: [copy], intentIdentifiers: [],
                                   hiddenPreviewsBodyPlaceholder: placeholder, options: []),
            UNNotificationCategory(identifier: Self.categoryView, actions: [view], intentIdentifiers: [],
                                   hiddenPreviewsBodyPlaceholder: placeholder, options: []),
            UNNotificationCategory(identifier: Self.categoryPlain, actions: [], intentIdentifiers: [],
                                   hiddenPreviewsBodyPlaceholder: placeholder, options: []),
        ])
    }

    func show(_ item: TomiItem) {
        let content = IncomingNotificationContent.forItem(item)
        let notification = UNMutableNotificationContent()
        notification.title = content.titleArgument.map { L10n.text(content.titleKey, $0) } ?? L10n.text(content.titleKey)
        notification.body = switch content.body {
        case let .key(key): L10n.text(key)
        case let .text(text): text
        }
        notification.categoryIdentifier = switch content.secondary {
        case .copy: Self.categoryCopy
        case .view: Self.categoryView
        case nil: Self.categoryPlain
        }
        notification.threadIdentifier = content.group.rawValue
        notification.userInfo = [Self.itemIdKey: item.id]
        notification.sound = quietRuns > 0 ? nil : .default
        notification.interruptionLevel = .active
        if let image = Self.imageAttachment(item) { notification.attachments = [image] }
        let request = UNNotificationRequest(identifier: item.id, content: notification, trigger: nil)
        center.add(request)
    }

    /// "Nouvel appareil": another device joined the space (nothing of any content, only the name the user gave it). A tap
    /// opens the list of devices.
    func showDeviceAdded(_ device: Device) {
        let notification = UNMutableNotificationContent()
        notification.title = L10n.text("ios_notif_device_added_title")
        notification.body = L10n.text("ios_notif_device_added_body", String(device.name.prefix(IncomingNotificationContent.maxBody)))
        notification.categoryIdentifier = Self.categoryPlain
        notification.threadIdentifier = "devices"
        notification.userInfo = [Self.opensDevicesKey: true]
        notification.sound = .default
        notification.interruptionLevel = .active
        center.add(UNNotificationRequest(identifier: "device-added-\(device.id)", content: notification, trigger: nil))
    }

    func cancel(_ itemIds: [String]) {
        guard !itemIds.isEmpty else { return }
        center.removeDeliveredNotifications(withIdentifiers: itemIds)
        center.removePendingNotificationRequests(withIdentifiers: itemIds)
    }

    /// The thumbnail the item carries (never a hidden or unreadable item's), made smaller for the notification: shown
    /// beside it and when it is opened, with no download. iOS keeps its own copy (it moves the file): the temporary one
    /// is gone at once.
    private static func imageAttachment(_ item: TomiItem) -> UNNotificationAttachment? {
        guard item.isReadable, !item.secret,
              let attachment = item.attachments.first(where: { $0.thumbnail != nil }), let data = attachment.thumbnail,
              let small = NotificationImage.make(data)
        else { return nil }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("notification-\(UUID().uuidString).jpg")
        do {
            // Written while the phone may be locked (a sync in the background): iOS moves it to its own store at once.
            try small.write(to: url, options: [.completeFileProtectionUntilFirstUserAuthentication])
            return try UNNotificationAttachment(identifier: "thumbnail", url: url, options: nil)
        } catch {
            try? FileManager.default.removeItem(at: url)
            return nil
        }
    }

    func cancelAll() {
        center.removeAllDeliveredNotifications()
        center.removeAllPendingNotificationRequests()
    }
}

/// What the user did with a notification: a tap (`nil` action), or one of its buttons.
struct NotificationResponse: Equatable {
    let itemId: String
    let action: String?
    /// The "Nouvel appareil" notification (no item): opens the devices screen.
    var opensDevices = false
}

/// The delegate of the notification center: set at launch (a tap may start the app), it hands the response to the app
/// as soon as the app is ready, and keeps notifications away while the app is on screen.
final class NotificationRouter: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let shared = NotificationRouter()

    @MainActor private var pending: NotificationResponse?
    @MainActor var handler: ((NotificationResponse) -> Void)? {
        didSet {
            if let handler, let pending {
                self.pending = nil
                handler(pending)
            }
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let userInfo = response.notification.request.content.userInfo
        let routed: NotificationResponse
        if userInfo[IncomingNotifications.opensDevicesKey] as? Bool == true {
            // A dismissal is not a tap.
            guard response.actionIdentifier == UNNotificationDefaultActionIdentifier else { return }
            routed = NotificationResponse(itemId: "", action: nil, opensDevices: true)
        } else {
            guard let itemId = userInfo[IncomingNotifications.itemIdKey] as? String else { return }
            let action: String? = switch response.actionIdentifier {
            case UNNotificationDefaultActionIdentifier, UNNotificationDismissActionIdentifier: nil
            default: response.actionIdentifier
            }
            routed = NotificationResponse(itemId: itemId, action: action)
        }
        await MainActor.run {
            if let handler { handler(routed) } else { pending = routed }
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        #if os(macOS)
        // Posted only while no surface of Tomi is used; Tomi may still be the active app (its settings in front),
        // where macOS would keep the notification silent. Not over a surface opened since it was posted.
        await MainActor.run { AppContainer.current?.surfaceVisible == true } ? [] : [.banner, .list, .sound]
        #else
        // On screen, the feed already shows it.
        []
        #endif
    }
}

#if os(iOS)
/// Sets the notification delegate at launch (iOS; the Mac app does it in `MacAppDelegate`).
final class TomiAppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        UNUserNotificationCenter.current().delegate = NotificationRouter.shared
        BackgroundWork.register()
        PushRegistration.start()
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        PushRegistration.received(deviceToken)
    }

    /// No token (no network, a simulator): the app works as before, fetching when iOS lets it.
    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {}

    func application(
        _ application: UIApplication, didReceiveRemoteNotification userInfo: [AnyHashable: Any]
    ) async -> UIBackgroundFetchResult {
        // An alert rang; a background wake-up did not.
        let rang = (userInfo["aps"] as? [AnyHashable: Any])?["alert"] != nil
        return await PushRegistration.woken(rang: rang)
    }
}
#endif

/// The notification size of a thumbnail: 300 px on its longest side, JPEG (a notification shows it small, and its
/// expanded view needs no more).
enum NotificationImage {
    static let maxPixels = 300

    static func make(_ thumbnail: Data) -> Data? {
        guard let source = CGImageSourceCreateWithData(thumbnail as CFData, nil) else { return nil }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixels,
        ]
        guard let decoded = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary) else { return nil }
        // On white: a transparent PNG would turn black in JPEG.
        guard let context = CGContext(
            data: nil, width: decoded.width, height: decoded.height, bitsPerComponent: 8, bytesPerRow: 0,
            space: CGColorSpace(name: CGColorSpace.sRGB)!, bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue
        ) else { return nil }
        context.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: decoded.width, height: decoded.height))
        context.draw(decoded, in: CGRect(x: 0, y: 0, width: decoded.width, height: decoded.height))
        guard let image = context.makeImage() else { return nil }
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(data, UTType.jpeg.identifier as CFString, 1, nil) else { return nil }
        CGImageDestinationAddImage(destination, image, [kCGImageDestinationLossyCompressionQuality: 0.8] as CFDictionary)
        return CGImageDestinationFinalize(destination) ? data as Data : nil
    }
}
