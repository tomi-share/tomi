import SwiftUI
#if os(iOS)
import UIKit
#else
import AppKit
import SystemConfiguration
#endif

// What iOS and macOS name differently, under one name, so that the screens shared by the two apps read the same.
// Nothing here changes what the iOS app does.

#if os(iOS)
typealias PlatformImage = UIImage
typealias PlatformFont = UIFont
#else
typealias PlatformImage = NSImage
typealias PlatformFont = NSFont

extension NSImage {
    /// As `UIImage(cgImage:)`: the size of the image is the one of its pixels.
    convenience init(cgImage: CGImage) {
        self.init(cgImage: cgImage, size: NSSize(width: cgImage.width, height: cgImage.height))
    }
}
#endif

extension Image {
    init(platformImage: PlatformImage) {
        #if os(iOS)
        self.init(uiImage: platformImage)
        #else
        self.init(nsImage: platformImage)
        #endif
    }
}

/// Calls to the system that differ between the two platforms.
@MainActor
enum SystemActions {
    /// Opens a link or a file with what the system chooses for it (browser, default app).
    static func open(_ url: URL) {
        #if os(iOS)
        UIApplication.shared.open(url)
        #else
        NSWorkspace.shared.open(url)
        #endif
    }

    /// The name of this device as the user gave it, proposed as its name in the space.
    static var deviceName: String {
        #if os(iOS)
        UIDevice.current.name
        #else
        (SCDynamicStoreCopyComputerName(nil, nil) as String?) ?? "Mac"
        #endif
    }

    /// The picture of this device (a phone, a computer).
    static var thisDeviceIcon: Image {
        #if os(iOS)
        Image(systemName: "iphone")
        #else
        Image(systemName: "desktopcomputer")
        #endif
    }

    /// An image has two places to be saved to (iOS: Photos, and Files). On the Mac both would write the same file to
    /// the same folder: one "Enregistrer" only.
    static var savesImagesInTwoPlaces: Bool {
        #if os(iOS)
        true
        #else
        false
        #endif
    }

    /// Where the user allows the camera again after a refusal (iOS: the settings of the app; macOS: the Camera
    /// page of Privacy & Security).
    static var cameraSettingsURL: URL? {
        #if os(iOS)
        URL(string: UIApplication.openSettingsURLString)
        #else
        URL(string: "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera")
        #endif
    }
}

extension ToolbarItemPlacement {
    /// The leading end of the bar of the screen (iOS: the navigation bar; macOS: the window toolbar).
    static var tomiLeading: ToolbarItemPlacement {
        #if os(iOS)
        .topBarLeading
        #else
        .navigation
        #endif
    }

    /// The trailing end of the bar of the screen.
    static var tomiTrailing: ToolbarItemPlacement {
        #if os(iOS)
        .topBarTrailing
        #else
        .primaryAction
        #endif
    }
}

// The bar of a screen on macOS. The screens shared with iOS draw their own buttons, so the Mac gives every button
// the plain style (`macScreenDefaults`). In a window toolbar that style squeezes a button to its symbol, with none of
// the padding the system gives a toolbar button. So: the navigation stack and what a screen puts in the bar go back
// to the style of the system, and the content of each screen keeps the plain one.
extension View {
    /// On a navigation stack. macOS: its bar (the back button) is drawn by the system.
    @ViewBuilder
    func systemBarControls() -> some View {
        #if os(macOS)
        buttonStyle(.automatic).menuStyle(.automatic)
        #else
        self
        #endif
    }

    /// On the content of a screen of a stack, its root as well as each destination. macOS: the plain style again.
    @ViewBuilder
    func screenControls() -> some View {
        #if os(macOS)
        macScreenDefaults()
        #else
        self
        #endif
    }

    /// The title of the window while a screen that gives none on iOS is shown (the feed, the code screen, the welcome
    /// page): the name of the app. Without it the window keeps the title of the screen shown before. iOS: nothing.
    @ViewBuilder
    func appWindowTitle() -> some View {
        #if os(macOS)
        navigationTitle(Text(verbatim: L10n.text("app_name")))
        #else
        self
        #endif
    }

    /// On a button or a menu that a screen puts in the bar. macOS: a toolbar button of the system (its padding, its
    /// hover), not the bare symbol.
    @ViewBuilder
    func barControl() -> some View {
        #if os(macOS)
        buttonStyle(.automatic).menuStyle(.automatic)
        #else
        self
        #endif
    }
}

extension View {
    /// The title of the screen in the bar, small (iOS). The window toolbar of macOS has one size only.
    @ViewBuilder
    func inlineNavigationTitle() -> some View {
        #if os(iOS)
        navigationBarTitleDisplayMode(.inline)
        #else
        self
        #endif
    }

    /// No navigation bar on this screen: it draws its own header (macOS: no window toolbar).
    @ViewBuilder
    func hiddenNavigationBar() -> some View {
        #if os(iOS)
        toolbar(.hidden, for: .navigationBar)
        #else
        toolbar(.hidden, for: .windowToolbar)
        #endif
    }

    /// The keyboard of a web address (iOS); a Mac has one keyboard.
    @ViewBuilder
    func urlKeyboard() -> some View {
        #if os(iOS)
        keyboardType(.URL)
        #else
        self
        #endif
    }

    /// The on-screen keyboard never capitalises (codes, tokens). A Mac keyboard never does.
    @ViewBuilder
    func noAutocapitalization() -> some View {
        #if os(iOS)
        textInputAutocapitalization(.never)
        #else
        self
        #endif
    }

    /// Sentences capitalised or not at all, on the on-screen keyboard.
    @ViewBuilder
    func autocapitalization(sentences: Bool) -> some View {
        #if os(iOS)
        textInputAutocapitalization(sentences ? .sentences : .never)
        #else
        self
        #endif
    }

    /// The plain ASCII on-screen keyboard (codes, hidden contents) when `ascii`. Nothing to choose on a Mac.
    @ViewBuilder
    func asciiKeyboard(_ ascii: Bool = true) -> some View {
        #if os(iOS)
        keyboardType(ascii ? .asciiCapable : .default)
        #else
        self
        #endif
    }

    /// A screen over the whole app (iOS: `fullScreenCover`). On macOS, which has none: over the whole window.
    @ViewBuilder
    func coverScreen<Item: Identifiable, Cover: View>(
        item: Binding<Item?>, @ViewBuilder content: @escaping (Item) -> Cover
    ) -> some View {
        #if os(iOS)
        fullScreenCover(item: item, content: content)
        #else
        // What is covered is hidden, not only drawn under: the scroll bars of the feed are AppKit views, which stay
        // above what SwiftUI draws, and nothing under a cover must take a click or the keyboard.
        opacity(item.wrappedValue == nil ? 1 : 0)
            .allowsHitTesting(item.wrappedValue == nil)
            .accessibilityHidden(item.wrappedValue != nil)
            // The toolbar of the screen under the cover (its back button) goes too: the cover is the whole window.
            .toolbar(item.wrappedValue == nil ? .automatic : .hidden, for: .windowToolbar)
            .overlay {
                if let value = item.wrappedValue {
                    content(value)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .transition(.opacity)
                }
            }
        #endif
    }
}
