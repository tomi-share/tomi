import AppKit
import Carbon.HIToolbox

/// A key with its modifiers, as the system hot key API takes them (Carbon values).
struct MacShortcut: Equatable {
    var keyCode: UInt32
    var modifiers: UInt32

    /// Control-Shift-Space: the fingers of the Windows shortcut (Ctrl+Shift+Space), free by default on macOS.
    static let standard = MacShortcut(keyCode: UInt32(kVK_Space), modifiers: UInt32(controlKey | shiftKey))

    /// From a key press. A global shortcut takes its keys away from every app, so it needs Control, or two of
    /// Command, Option and Control: Command alone would take Command-V or Command-Q everywhere, Option alone the
    /// characters typed with Option (`{`, `€` on a French keyboard). A function key may come alone.
    init?(event: NSEvent) {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        var modifiers: UInt32 = 0
        var strong = 0
        if flags.contains(.command) { modifiers |= UInt32(cmdKey); strong += 1 }
        if flags.contains(.option) { modifiers |= UInt32(optionKey); strong += 1 }
        if flags.contains(.control) { modifiers |= UInt32(controlKey); strong += 2 }
        let functionKey = Self.functionKeys.contains(Int(event.keyCode))
        guard strong >= 2 || functionKey else { return nil }
        if flags.contains(.shift) { modifiers |= UInt32(shiftKey) }
        self.init(keyCode: UInt32(event.keyCode), modifiers: modifiers)
    }

    private static let functionKeys: Set<Int> = [
        kVK_F1, kVK_F2, kVK_F3, kVK_F4, kVK_F5, kVK_F6, kVK_F7, kVK_F8, kVK_F9, kVK_F10, kVK_F11, kVK_F12,
    ]

    init(keyCode: UInt32, modifiers: UInt32) {
        self.keyCode = keyCode
        self.modifiers = modifiers
    }

    /// As macOS writes shortcuts: the modifiers in the system order, then the key.
    var display: String {
        var text = ""
        if modifiers & UInt32(controlKey) != 0 { text += "⌃" }
        if modifiers & UInt32(optionKey) != 0 { text += "⌥" }
        if modifiers & UInt32(shiftKey) != 0 { text += "⇧" }
        if modifiers & UInt32(cmdKey) != 0 { text += "⌘" }
        return text + Self.keyName(keyCode)
    }

    private static var named: [Int: String] { [
        kVK_Space: L10n.text("macos_key_space"), kVK_Return: "↩", kVK_Tab: "⇥", kVK_Delete: "⌫", kVK_ForwardDelete: "⌦", kVK_Escape: "⎋",
        kVK_LeftArrow: "←", kVK_RightArrow: "→", kVK_UpArrow: "↑", kVK_DownArrow: "↓",
        kVK_Home: "↖", kVK_End: "↘", kVK_PageUp: "⇞", kVK_PageDown: "⇟",
        kVK_F1: "F1", kVK_F2: "F2", kVK_F3: "F3", kVK_F4: "F4", kVK_F5: "F5", kVK_F6: "F6", kVK_F7: "F7", kVK_F8: "F8",
        kVK_F9: "F9", kVK_F10: "F10", kVK_F11: "F11", kVK_F12: "F12",
    ] }

    /// The character the key gives on the current keyboard layout (an AZERTY "A" is not the key of a QWERTY "A").
    private static func keyName(_ keyCode: UInt32) -> String {
        if let name = named[Int(keyCode)] { return name }
        guard let source = TISCopyCurrentASCIICapableKeyboardLayoutInputSource()?.takeRetainedValue(),
              let raw = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else { return "?" }
        let layout = Unmanaged<CFData>.fromOpaque(raw).takeUnretainedValue() as Data
        var dead: UInt32 = 0
        var length = 0
        var characters = [UniChar](repeating: 0, count: 4)
        let status = layout.withUnsafeBytes { bytes -> OSStatus in
            guard let pointer = bytes.bindMemory(to: UCKeyboardLayout.self).baseAddress else { return -1 }
            return UCKeyTranslate(
                pointer, UInt16(keyCode), UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()),
                OptionBits(kUCKeyTranslateNoDeadKeysBit), &dead, characters.count, &length, &characters
            )
        }
        guard status == noErr, length > 0 else { return "?" }
        return String(utf16CodeUnits: characters, count: length).uppercased()
    }
}

/// One global shortcut of Tomi, pressed in any app. Two exist (`Kind`): one opens Tomi (or closes it), the other
/// sends what is selected in the app in front. The system hot key API needs no accessibility permission and works in
/// the sandbox; it never sees what is typed elsewhere.
@MainActor
final class MacGlobalShortcut {
    enum Kind: UInt32, CaseIterable {
        /// Opens the popover (or closes it): Control-Shift-Space.
        case open = 1
        /// Copies the selection of the app in front and opens the send screen with it: Control-Shift-V.
        case sendSelection = 2

        var standard: MacShortcut {
            switch self {
            case .open: .standard
            case .sendSelection: MacShortcut(keyCode: UInt32(kVK_ANSI_V), modifiers: UInt32(controlKey | shiftKey))
            }
        }

        /// Where the choice is kept (the open shortcut keeps the keys it always had).
        fileprivate var keyCodeKey: String { self == .open ? "globalShortcutKeyCode" : "sendShortcutKeyCode" }
        fileprivate var modifiersKey: String { self == .open ? "globalShortcutModifiers" : "sendShortcutModifiers" }
    }

    nonisolated fileprivate static let signature: OSType = 0x546F_6D69 // "Tomi"

    let kind: Kind
    private let defaults: UserDefaults
    private var hotKey: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let onPressed: () -> Void
    private(set) var current: MacShortcut
    /// The other shortcut of Tomi: the same keys cannot do both.
    weak var other: MacGlobalShortcut?

    init(_ kind: Kind = .open, defaults: UserDefaults = .standard, onPressed: @escaping () -> Void) {
        self.kind = kind
        self.defaults = defaults
        self.onPressed = onPressed
        if defaults.object(forKey: kind.keyCodeKey) != nil {
            current = MacShortcut(
                keyCode: UInt32(defaults.integer(forKey: kind.keyCodeKey)),
                modifiers: UInt32(defaults.integer(forKey: kind.modifiersKey))
            )
        } else {
            current = kind.standard
        }
        installHandler()
        // A saved shortcut another app took meanwhile: the standard one.
        if !register(current), current != kind.standard, register(kind.standard) { current = kind.standard }
    }

    /// Nothing is registered (the system refused the saved shortcut and the standard one).
    var isRegistered: Bool { hotKey != nil }

    /// While a new shortcut is being recorded: the current one must reach the recorder, not act.
    func suspend() {
        unregister()
    }

    func resume() {
        if hotKey == nil { _ = register(current) }
    }

    /// Changes the shortcut. False when the system refuses it (another app registered the same hot key; the
    /// shortcuts of the system and of the menus of other apps are not seen here) or when it is the other shortcut of
    /// Tomi: the previous one stays.
    func set(_ shortcut: MacShortcut) -> Bool {
        guard shortcut != current else { return true }
        if let other, other.current == shortcut { return false }
        let previous = current
        unregister()
        guard register(shortcut) else {
            _ = register(previous)
            return false
        }
        current = shortcut
        defaults.set(Int(shortcut.keyCode), forKey: kind.keyCodeKey)
        defaults.set(Int(shortcut.modifiers), forKey: kind.modifiersKey)
        return true
    }

    private func register(_ shortcut: MacShortcut) -> Bool {
        var reference: EventHotKeyRef?
        let id = EventHotKeyID(signature: Self.signature, id: kind.rawValue)
        let status = RegisterEventHotKey(shortcut.keyCode, shortcut.modifiers, id, GetApplicationEventTarget(), 0, &reference)
        guard status == noErr, let reference else { return false }
        hotKey = reference
        return true
    }

    private func unregister() {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        hotKey = nil
    }

    /// The delegate keeps this object for the whole run; released earlier, it takes its registrations with it (the
    /// handler holds an unretained pointer to it).
    deinit {
        if let hotKey { UnregisterEventHotKey(hotKey) }
        if let handler { RemoveEventHandler(handler) }
    }

    /// Every hot key of the app reaches every handler: each one takes only the presses of its own shortcut and lets
    /// the others go on.
    private func installHandler() {
        var type = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let pointer = Unmanaged.passUnretained(self).toOpaque()
        InstallEventHandler(GetApplicationEventTarget(), { _, event, userData in
            guard let userData, let event else { return OSStatus(eventNotHandledErr) }
            var pressed = EventHotKeyID()
            let status = GetEventParameter(
                event, EventParamName(kEventParamDirectObject), EventParamType(typeEventHotKeyID), nil,
                MemoryLayout<EventHotKeyID>.size, nil, &pressed
            )
            let shortcut = Unmanaged<MacGlobalShortcut>.fromOpaque(userData).takeUnretainedValue()
            // Hot key events are delivered on the main thread.
            return MainActor.assumeIsolated {
                guard status == noErr, pressed.signature == MacGlobalShortcut.signature, pressed.id == shortcut.kind.rawValue else {
                    return OSStatus(eventNotHandledErr)
                }
                shortcut.onPressed()
                return noErr
            }
        }, 1, &type, pointer, &handler)
    }
}
