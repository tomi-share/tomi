// Every string shown in the webview, in English. Rust-side strings (tray, toasts) live in strings.rs.
// Plain "you" everywhere; "Hidden" / "Show" (never "Secret" / "Reveal").

import type {
  ContentKind,
  ExpiryChoice,
  ItemFilter,
  Rejection,
  RenderMode,
  TransferPhase,
} from "../api";

export const en = {
  appName: "Tomi",
  empty: {
    all: "Nothing yet. Write, paste or drop something above to send it to your devices.",
    receiveOnly: "Nothing yet.",
    filtered: "No matching items.",
    // Android's text says "Swipe a card from Recent to the left": there is no swipe on a PC.
    archives: "No archived items. Choose “Archive” in the menu of an item in Recent to put it away here.",
    search: "No results. Hidden content is searched by device only, never by title or by content.",
  },
  searchResults: {
    one: "{n} result",
    other: "{n} results",
  },
  // Next to the counter, only when a search has a hidden item among its results.
  searchMaskedNote: "hidden items are searched without their title or content",
  sections: { pinned: "Pinned", recent: "Recent", archives: "Archives" },
  tabs: { history: "History", settings: "Settings", devices: "Devices", onboarding: "Welcome" },
  sectionsLabel: "Sections",
  filtersLabel: "Filters",
  filters: {
    all: "All",
    links: "Links",
    text: "Text",
    files: "Files",
    secrets: "Hidden",
    archives: "Archives",
  } satisfies Record<ItemFilter, string>,
  kinds: {
    link: "Link",
    text: "Text",
    richText: "Rich text",
    image: "Image",
    file: "File",
    files: "Files",
  } satisfies Record<ContentKind, string>,
  // Two modes only, like Android: "Preview" (styled) and "Markdown" (the literal source). "Markdown"
  // exists only for an item that has a Markdown representation. HTML is never shown as source.
  renderModes: { preview: "Preview", source: "Markdown" } satisfies Record<RenderMode, string>,
  renderModesLabel: "Text display",
  expiry: {
    never: "Never",
    afterFirstRead: "After reading",
    hour1: "1 h",
    day1: "1 d",
    day7: "7 d",
    day30: "30 d",
  } satisfies Record<ExpiryChoice, string>,
  composer: {
    label: "Send",
    placeholder: "Write, paste a link or a text, drop files…",
    placeholderSecret: "Value to hide…",
    title: "Title (optional)",
    titleSecret: "Hidden title (optional)",
    url: "Link",
    secret: "Hide content",
    secretOff: "Preview hidden everywhere until “Show”",
    secretHint:
      "Encrypted, hidden on screen (title included), cleared from the clipboard after 30 s.",
    showInput: "Show input",
    hideInput: "Hide input",
    removeField: "Remove",
    removeFile: "Remove {name}",
    addFiles: "Attach a file",
    expires: "Expires",
    afterReadHint: "“After reading”: erased everywhere as soon as it is first opened.",
    afterReadNoFiles: "“After reading” is not offered with files: they would be erased before they could be opened.",
    send: "Send",
    drop: "Drop to add",
    looksSecret: "This looks like a password. Hide it?",
    markSecret: "Hide",
    sourceFormat: "Detected format",
    to: "To {devices}",
    prefillKept: "The link or title received was not added: the field was already filled in.",
  },
  job: {
    pending: "Sending in",
    cancel: "Cancel",
    sending: "Sending…",
    done: "Sent",
    queued: "Saved, it will be sent as soon as the network is back",
    cancelled: "Send canceled",
    failed: "Send failed",
    retry: "Try again",
    dismiss: "Discard",
  },
  card: {
    copy: "Copy",
    copyLink: "Copy link",
    copied: "Copied",
    copiedSecret: "Copied · cleared in 30 s",
    copiedAndGone: "Copied. This content is now erased from all your devices.",
    open: "Open",
    save: "Save",
    saved: "Saved to {dir}",
    showFolder: "Show folder",
    reveal: "Show",
    hide: "Hide",
    details: "Details",
    readMore: "Read more",
    more: "More actions",
    delete: "Delete",
    deleteLocal: "Delete from this device",
    deleteEverywhere: "Delete everywhere",
    deleteLocalTitle: "Delete from this device?",
    deleteLocalMessage: "The item will be removed from this device only. Your other devices keep their copy. This action is permanent.",
    deletedLocal: "Deleted from this device",
    deletedEverywhere: "Deletion requested on all devices",
    // Removal of ONE file of an item (the item stays), same words as Android.
    imageDeletedLocal: "Image deleted from this device",
    imageDeletedEverywhere: "Deletion of the image requested on all devices",
    fileDeletedLocal: "File deleted from this device",
    fileDeletedEverywhere: "Deletion of the file requested on all devices",
    fileActions: "Actions for this file",
    fileDeleteLocal: "Delete the file from this device",
    fileDeleteEverywhere: "Delete the file everywhere",
    fileDeleteTitle: "Delete the file everywhere?",
    fileDeleteMessage:
      "“{name}” will be erased on all your devices and on the relay. Offline devices will erase it when they reconnect. This cannot be undone.",
    fileDeleteSecretMessage:
      "This hidden file will be erased on all your devices and on the relay. A device that has already shown it may have kept a copy. This cannot be undone.",
    secret: "Hidden",
    pin: "Pin",
    unpin: "Unpin",
    pinned: "Pinned",
    // Archive (spec/archive-contract.md): same words as Android (strings.xml).
    archive: "Archive",
    unarchive: "Unarchive",
    archived: "Archived",
    archivedToast: "Archived",
    unarchivedToast: "Unarchived",
    undo: "Undo",
    undoKey: "Ctrl+Z",
    archiveFailed: "Couldn't change the archive. Try again.",
    viewImage: "View image",
    viewCode: "Open file",
    fileHidden: "1 hidden file",
    filesHidden: "{n} hidden files",
    // "N more files ›" (button to the detail), then the same as a caption when "Read more" takes its place.
    moreFiles: { one: "{n} more file", other: "{n} more files" },
    moreFilesCaption: { one: "+ {n} more file", other: "+ {n} more files" },
    // Count and total size above the attachments ("4 files · 4.9 MB"); from 2 attachments on.
    summaryFiles: "{n} files · {size}",
    summaryImages: "{n} images · {size}",
    fileGone: "unavailable",
    characters: { one: "{n} character", other: "{n} characters" },
    revealedHint: "Hides again as soon as you leave the window.",
    unreadable:
      "This item is unreadable for now. It is kept: a sync or an update may make it readable.",
    unreadableUpdate:
      "An update is needed to read this item. It is kept and will be shown after Tomi is updated.",
    unreadableShort: "Unreadable",
    thisPc: "This PC",
    from: "{device}",
    toDevices: "to your devices",
    expiresIn: "expires in {when}",
    expired: "expired",
    afterRead: "disappears after the first read",
    dragOut: "Drag the file out of the window",
    new: "Unread",
    yesterday: "Yesterday",
    // Delivery status of a sent item (summary of the per-device states, like Android).
    status: {
      queued: "Waiting to send",
      sent: "Sent",
      failed: "Send failed",
      deliveredAll: "Delivered to all",
      deliveredSome: "Delivered to {delivered} of {total}",
      expired: "Delivered to {delivered} · expired before delivery to {devices}",
    },
    deleteTitle: "Delete everywhere?",
    deleteMessage:
      "The item will be erased from all your devices and from the relay. Offline devices will erase it when they reconnect. This action is permanent.",
    deleteSecretMessage:
      "This hidden content will be erased from all your devices and from the relay. A device that has already shown it may have kept a copy. This action is permanent.",
  },
  detail: {
    title: "Details",
    back: "Back",
    notFound: "Item not found. It may have expired or been deleted.",
    maskedHint: "The content stays hidden until you press “Show”.",
    actions: "Item actions",
    info: "Information",
    deleteMenu: "Delete…",
    deleteTitle: "Delete this item?",
    deleteMessage:
      "Deleting from this device does not affect your other devices. Deleting everywhere erases the item from all your devices and from the relay; offline devices will erase it when they reconnect. This is permanent.",
    deleteSecretMessage:
      "Deleting from this device does not affect your other devices. Deleting everywhere erases this hidden content from all your devices and from the relay; a device that has already shown it may have kept a copy. This is permanent.",
    // Shown in the tooltips and next to "Back".
    keys: { back: "Esc", copy: "Ctrl+C", open: "Ctrl+O", save: "Ctrl+S", delete: "Del" },
    meta: {
      type: "Type",
      sentAt: "Sent on",
      expiry: "Expires",
      pinned: "Pinned",
      status: "Status",
      yes: "Yes",
      no: "No",
      never: "Never",
      afterRead: "After reading",
      received: "Received",
    },
  },
  viewer: {
    close: "Close",
    previous: "Previous image",
    next: "Next image",
    position: "{n}/{total}",
    description: "Image {n} of {total}",
    zoomIn: "Zoom in",
    zoomOut: "Zoom out",
    zoomReset: "Actual size",
    fit: "Fit to window",
    delete: "Delete",
    deleteLocal: "Delete the image from this device",
    deleteEverywhere: "Delete the image everywhere",
    deleteTitle: "Delete the image everywhere?",
    deleteMessage:
      "This image will be erased on all your devices and on the relay. Offline devices will erase it when they reconnect. This cannot be undone.",
    deleteSecretMessage:
      "This hidden image will be erased on all your devices and on the relay. A device that has already shown it may have kept a copy. This cannot be undone.",
    loadFailed: "Couldn't show this image.",
    tooLarge: "This image is too large to be shown here. Open it with “Open”.",
    loading: "Loading the image…",
    hint: "Scroll wheel to zoom, drag to move, arrow keys to change image.",
  },
  code: {
    title: "File",
    search: "Search in file",
    searchPlaceholder: "Search in file",
    previous: "Previous match",
    next: "Next match",
    copyFile: "Copy file",
    copied: "File copied",
    wrap: "Wrap lines",
    close: "Close",
    tooLarge: "This file is too large to be shown (2 MB maximum).",
    binary: "This file isn't text: it can't be shown here.",
    unavailable: "Couldn't show this file right now.",
    noColoursLanguage: "No syntax highlighting for this language.",
    noColoursSize: "Syntax highlighting unavailable: file too long (50 KB or 2,000 lines maximum).",
    position: "{n}/{total}",
    lines: { one: "{n} line", other: "{n} lines" },
    loading: "Loading the file…",
  },
  status: {
    notPaired: "This PC is not paired with your Tomi space yet. You will be able to send and receive as soon as it is.",
    // The demo space is the active space (same sentences as iOS and Android).
    demo: "Demo mode: sample data, nothing is sent.",
    demoLeave: "Leave the demo",
    revoked: "This PC has been removed from your space. Nothing can be sent to it or read on it anymore.",
    updateRequired: "Your space uses a newer version of Tomi than this one. Update Tomi to continue.",
    blocked: "Your space is blocked for now. Try again later.",
    notFound: "Your Tomi server can no longer find your space. Nothing has been erased on this PC.",
    storage: "Tomi's local storage cannot be accessed. Your data has not been touched.",
    storageCodes: {
      wrap_key_missing:
        "The key that protects the data (created by Windows) cannot be found: the data is intact but unreadable.",
      wrap_key_unreadable:
        "Windows cannot open the key that protects the data: the data comes from another account or another PC.",
    } as Record<string, string>,
    seeDevices: "Devices",
    // The removal of a device is not finished (the key rotation keeps failing and is retried
    // every hour): calm, no alarm, and what helps.
    rotationStalledTitle: "Device removal in progress",
    rotationStalled:
      "A removed device may still hold the current key of your space. Tomi retries automatically. Opening Tomi on your other devices can help.",
  },
  // A device the relay removed on its own (said once, without blocking anything).
  relayNotice: {
    admin: "{names} was removed by the administrator of the relay.",
    adminMany: "{names} were removed by the administrator of the relay.",
    self: "{names} left the space.",
    selfMany: "{names} left the space.",
    other: "{names} was removed from your space.",
    otherMany: "{names} were removed from your space.",
  },
  popover: {
    pin: "Keep open",
    unpin: "Stop keeping open",
    history: "Full history",
    settings: "Settings",
    search: "Search",
  },
  settings: {
    title: "Settings",
    shortcut: "Global shortcut",
    shortcutHelp: "Opens Tomi from anywhere. Example: Ctrl+Shift+Space.",
    shortcutApply: "Apply",
    shortcutSaved: "Shortcut saved: {shortcut}",
    shortcutNotRegistered: "The shortcut is not active (already used by another application?).",
    autostart: "Start Tomi when Windows starts",
    notifications: "Notifications when something is received",
    theme: "Theme",
    themeHelp: "Follows the Windows theme, or stays light or dark.",
    themes: { system: "System", light: "Light", dark: "Dark" },
    downloadDir: "Save folder",
    downloadDirDefault: "Downloads\\Tomi",
    downloadDirIsDefault: "Default",
    choose: "Choose…",
    reset: "Default",
    about: "Tomi {version}",
    quit: "Quit Tomi",
  },
  // Leaving the space, deleting it, erasing without the server's proof. Same wording as Android
  // (strings.xml), "phone" becoming "PC".
  departure: {
    sectionTitle: "My space",
    sectionHelp:
      "Disconnecting removes this PC from your space and erases everything it holds. Your other devices keep their copy.",
    unavailable: "Disconnecting is not available yet in this version of Tomi.",
    cancel: "Cancel",
    retry: "Try again",
    // Disconnect
    leave: "Disconnect",
    leaveTitle: "Disconnect from your space?",
    leaveMessage:
      "This PC will leave your space. Everything it holds will be erased: items, hidden content, files and keys, including what has not been sent yet. Your other devices keep their copy. To come back, you will have to add this PC again from another device.",
    leaveInProgress: "Disconnecting…",
    leaveErrorUpdate: "Update Tomi to be able to disconnect. Nothing has been erased.",
    leaveErrorUnexpected: "Your Tomi server refused the disconnection. Nothing has been erased.",
    // Delete the space (the last device)
    deleteTitle: "Delete your space?",
    deleteMessage:
      "This is the only device. Disconnecting deletes your space and everything in it, permanently.",
    deleteDetail:
      "Your Tomi server erases the space, then this PC erases everything it holds: items, hidden content, files and keys. No one will be able to recover them.",
    deleteConfirm: "Delete the space",
    deleteInProgress: "Deleting…",
    deleteErrorRelayTooOld:
      "Your Tomi server is too old to delete the space. Update it, then try again. Nothing has been erased.",
    deleteErrorUpdate: "Update Tomi to be able to delete your space. Nothing has been erased.",
    deleteErrorUnexpected:
      "Your Tomi server refused to delete the space. Nothing has been erased.",
    // The banner of a departure that waits
    pendingLeaveTitle: "Disconnection pending",
    pendingDeleteTitle: "Space deletion pending",
    pendingChecking: "Tomi is checking with your server. Nothing has been erased.",
    pendingNetwork:
      "Your Tomi server has not confirmed yet. Nothing has been erased. Tomi tries again automatically, at most once a minute, and when you come back to this window or the network is back.",
    pendingServer: "Your Tomi server has a temporary problem. Nothing has been erased. Tomi is trying again.",
    pendingNotSent:
      "Your request did not go through: this PC is still part of your space. Nothing has been erased.",
    pendingNotFound:
      "Your Tomi server can no longer find your space, or no longer recognizes this PC. Nothing has been erased.",
    wiping: "Erasing this PC…",
    cancelledLeave:
      "Disconnection canceled. If your server had already received it, this PC will appear as removed from the space; what it holds stays readable.",
    cancelledDelete:
      "Deletion canceled. If your server had already done it, your space now exists only on this PC.",
    failed: "Erasing did not complete. Nothing has been erased. Try again.",
    // States without a departure
    notFoundTitle: "Space not found",
    notFoundMessage:
      "Your Tomi server can no longer find your space. If it has been reset, you can erase this PC. Nothing has been erased on this PC.",
    revokedTitle: "Removed from the space",
    revokedMessage:
      "This PC has been removed from your space. What it holds stays readable here. Disconnecting erases it.",
    // Erase anyway
    eraseAnyway: "Erase anyway",
    eraseTitle: "Erase this PC without confirmation from the server?",
    eraseMessage:
      "Your Tomi server cannot confirm that your space is deleted. The space may not be deleted on your server; if it still exists, this PC will no longer have access to it.",
    eraseDetail:
      "Everything this PC holds will be permanently erased: items, hidden content, files and keys.",
    eraseCheckbox: "I understand: this is permanent.",
    eraseConfirm: "Erase this PC",
    eraseInProgress: "Erasing…",
    eraseChecking: "Tomi is checking with your server…",
    eraseNotAllowed: "Tomi must check with your server first. Try again in a moment.",
    // Windows Hello (the prompt itself is drawn by Windows)
    helloNote: "You will be asked for a Windows Hello verification.",
    authenticating: "Windows Hello verification in progress…",
    // Said to screen readers when the confirm button of an irreversible dialog is armed (2 s).
    ready: "You can now confirm.",
    // "Cancel" pressed while a request is on its way: it goes on to its end and erases nothing.
    cancellingTitle: "Canceling",
    cancellingText:
      "Tomi is waiting for the request already sent to your server to finish. Nothing will be erased.",
  },
  devices: {
    title: "Devices",
    thisDevice: "This device",
    unverified: "not verified",
    lastSeen: "seen {when}",
    never: "never seen",
    // Tomi only knows that the device has not shown up, not that it is uninstalled.
    inactive: "Inactive for {duration}. No longer using it? Remove it.",
    inactiveDuration: { one: "{n} day", other: "{n} days" },
    empty: "No other devices.",
    platforms: { windows: "Windows", android: "Android", other: "Other" },
    notPaired: "This PC is not paired yet: no devices to show.",
    // Rename and revoke (same wording as Android)
    more: "Actions for {device}",
    rename: "Rename",
    renameLabel: "Device name",
    renameHint: "This is the name your other devices will see (the relay too: do not put anything confidential in it).",
    renameSave: "Save",
    renameSaving: "Saving…",
    renamed: "Device renamed.",
    revoke: "Revoke",
    revokeTitle: "Revoke “{device}”?",
    revokeMessage:
      "This device will be removed from your space and the space key will be renewed: it will no longer be able to read new items. Windows Hello will ask you to confirm.",
    revoking: "Revoking…",
    revoked: "“{device}” has been revoked.",
    // Nothing waits for the network here (unlike a send). A timeout after the relay accepted the
    // call is also reported as offline, so the text does not claim that nothing happened.
    offline: "Offline: the operation may not have completed. Try again when the network is back.",
  },
  // Pairing (spec/pairing-v1.md). The code, its link and the verification code are shown
  // and never typed back, copied or logged.
  pairing: {
    // A PC that is not in a space
    create: "Create my space",
    createHint: "No other Tomi device? Create your space with the installation key.",
    relayUnknown: "The relay is not responding right now. Check your connection, Tomi tries again automatically.",
    back: "Back",
    deviceName: "Name of this PC",
    deviceNameHint: "This is the name your other devices will see.",
    pending: "An interrupted pairing is being checked…",
    resumedProvisioned: "The interrupted pairing is complete: this PC is part of your space.",
    resumedDiscarded:
      "The interrupted pairing did not complete: the other device had not added this PC yet. Start again with a new code.",
    ok: "OK",
    // The demo space of the core (same sentences as iOS and Android).
    demoEnter: "Try with sample data",

    // First device
    createTitle: "Create your space",
    createIntro:
      "Paste the installation key or the creation link. It is shown only once, when the person who runs the relay creates it (command {command} on the server). If it is lost, they create another one.",
    tokenLabel: "Installation key or creation link",
    tokenHint: "It is used only for this creation and is not kept. A creation link also gives the relay.",
    showInput: "Show input",
    hideInput: "Hide input",
    createDo: "Create the space",
    creating: "Creating the space…",
    created: "Your space is created. This PC is its first device.",

    // New device
    joinTitle: "Join your space",
    joinIntro:
      "On your other device, open Tomi › Devices › Add a device, then enter the short code or paste the link.",
    codeLabel: "Pairing code",
    codePlaceholder: "For example 7-crayon-renard",
    codeHint: "A code or a link. Capital letters and accents do not matter. Tab completes the suggested word.",
    linkReceived: "Pairing link received",
    linkReceivedHint: "Nothing is done automatically. Press “Join” to continue.",
    linkIgnored: "This PC is already part of a space: the pairing link received is ignored.",
    // The relay: read from a link or typed, never trusted by itself (shown in full, confirmed with the code)
    relayLabel: "Relay",
    relayPlaceholder: "https://relay.example.org",
    relayHint:
      "The address of your space's relay. It is taken from the link or the QR code when it contains one.",
    relayHintRequired:
      "This PC has no relay yet: enter its address, or paste a pairing link that contains it.",
    relayOther: "Other relay",
    relayFromLink: "Relay read from the link: check that it is yours.",
    relayUsed: "Relay: {relay}",
    relayNone: "No relay: enter its address.",
    relayVerify: "Check that this is your relay before continuing.",
    relayOnScreen: "Relay: {relay}",
    setupLinkReceived: "Space creation link received",
    setupLinkReceivedHint:
      "Nothing is done automatically. Check the relay, then press “Create the space”.",
    setupLinkPasted: "This text is a space creation link or key, not a pairing code.",
    setupLinkUse: "Create my space with",
    joinDo: "Join",
    joining: "Connecting to the other device…",
    waitingJoin: "Waiting for the other device…",
    working: "Pairing…",

    // Inviting a device
    addDevice: "Add a device",
    inviteTitle: "Add a device",
    inviteIntro:
      "On the new device, open Tomi › Join, then scan the QR code or enter the short code.",
    preparing: "Preparing the code…",
    qrLabel: "Pairing QR code",
    shortCode: "Short code",
    expiresIn: "Valid for another {time}",
    expired: "This code is expiring.",
    renewed: "The pairing did not complete. A new code is ready.",
    cancel: "Cancel",

    // Comparing the codes
    verifyTitle: "Check the code",
    verifyJoin: "Check that the same code is shown on {device}.",
    verifyInvite:
      "Check that {device} shows the same code. If it shows an error or a different code, refuse.",
    verifyHint: "If you wait too long, the pairing expires: you will start again with a new code.",
    codeDescription: "Verification code {code}",
    spaceSizeOne: "Your space has 1 device.",
    spaceSizeMany: "Your space has {n} devices.",
    same: "The codes are identical",
    different: "They are different",
    add: "Add {device}",
    refuse: "Refuse",
    historyTitle: "Retrieve the history?",
    historySize: "{items} items, {size}",
    historyYes: "Yes, retrieve the history",
    historyYesHint: "Everything in your space arrives here, including hidden content and files.",
    historyNo: "No, start from an empty feed",
    historyNoHint: "Only what is sent from now on will arrive here.",

    // After the decision
    addingTitle: "Adding {device}",
    joiningTitle: "Connecting to your space",
    provisioningInvite: "If you have not done so yet, also confirm the code on {device}.",
    provisioningJoin: "Waiting for the confirmation on {device}…",
    doneInvite: "{device} is part of your space.",
    doneJoin: "This PC is part of your space.",
    historyPreparingInvite:
      "The history is being prepared for {device}. You can follow the transfer below.",
    historyPreparingJoin: "The history of your space will arrive in a few moments.",
    finish: "Done",
    newCode: "New code",
    close: "Close",
    offline: "Network unavailable. The pairing resumes as soon as it is back.",
    // The same failure is told differently to the device that typed the code and to the one that
    // showed it (spec/pairing-v1.md §11.5). The one that showed it offers a new code itself.
    failedJoin: "Incorrect code. Generate a new code on the other device.",
    failedInvite: "The pairing did not complete.",
  },
  transfer: {
    title: "History",
    offer: "{device} is offering you its history.",
    offerBody:
      "Everything in its feed arrives here, including hidden content and files. You can answer no: you will start from an empty feed.",
    accept: "Yes, retrieve the history",
    decline: "No",
    send: "Transfer the history…",
    sendAsk:
      "Send the whole history to {device}? Including hidden content and files, sealed for that device only.",
    sendYes: "Yes, send",
    sendNo: "No",
    cancel: "Cancel",
    toDevice: "to {device}",
    fromDevice: "from {device}",
    progress: "{done} / {total} items",
    files: "{done} / {total} files",
    phases: {
      offered: "Offered",
      running: "In progress",
      done: "Done",
      failed: "Failed",
      cancelled: "Canceled",
      declined: "Declined",
    } satisfies Record<TransferPhase, string>,
  },
  onboarding: {
    title: "Welcome to Tomi",
    intro: "Send a text, a link, a password or a file to your other devices, and receive them the same way.",
    steps: [
      "Click the icon in the notification area, or use the shortcut {shortcut}.",
      "Paste or drop what you want to send, then confirm. You have a few seconds to cancel.",
      "Right-clicking a file > Send to > Tomi works too.",
    ],
    done: "Let's go",
  },
  common: { close: "Close", loading: "Loading…", cancel: "Cancel", ok: "OK" },
  time: {
    now: "just now",
    minutes: "{n} min ago",
    hours: "{n} h ago",
    days: "{n} d ago",
    inMinutes: "in {n} min",
    inHours: "in {n} h",
    inDays: "in {n} d",
  },
  rejected: {
    tooLarge: "{name}: over 25 MB, refused.",
    itemTotalExceeded: "{name}: the send would exceed 100 MB.",
    notAFile: "{name}: this is not a file.",
    tooMany: "{name}: too many files in a single send (32 at most).",
  } satisfies Record<Rejection["reason"], string>,
  errors: {
    "error.attachment_missing": "An attached file cannot be found.",
    "error.autostart": "Couldn't change automatic startup.",
    "error.blocked": "This PC can no longer use your space (removed, or Tomi needs to be updated).",
    "error.clipboard": "The clipboard is unavailable. Try again.",
    "error.clock": "The clock of this PC seems wrong: the pin or the archive could not be saved.",
    "error.core": "Internal error in the Tomi core.",
    "error.devices_changed": "The devices in your space changed during the operation. Try again.",
    "error.device_name_invalid":
      "This name is not valid: it must be 1 to 64 characters long, with no control characters.",
    "error.download_dir": "Save folder not found.",
    // Windows Hello (departure, revoking a device, showing or copying hidden content): asked in Rust, never bypassed
    "error.auth_cancelled": "Verification canceled. Nothing has been changed.",
    "error.auth_failed": "The Windows verification did not succeed (Windows Hello or account password). Nothing has been changed. Try again.",
    "error.auth_busy": "Windows Hello is busy. Try again in a moment. Nothing has been changed.",
    "error.strong_auth_unavailable":
      "No verification is possible on this PC: Windows Hello is unavailable and the password of your Windows account cannot be asked for. Nothing has been changed.",
    "error.strong_auth_no_password":
      "Windows Hello is unavailable and your Windows account has no password, so nothing can be verified. Set a password for your Windows account (or set up Windows Hello), then try again. Nothing has been changed.",
    "error.strong_auth_too_many":
      "Too many attempts with the password of your Windows account: wait a minute, then try again. Nothing has been changed.",
    "error.strong_auth_locked":
      "Your Windows account is locked (too many wrong passwords). Unlock it or wait, then try again. Nothing has been changed.",
    "error.departure_unavailable": "Disconnecting is not available yet in this version of Tomi.",
    "error.departure_state": "This window is out of date. Nothing has been erased.",
    "error.departure_busy": "An operation is already in progress.",
    "error.departure_not_armed": "Wait two seconds before confirming.",
    "error.erase_not_allowed": "Tomi must check with your server first. Try again in a moment.",
    "error.erase_not_checked": "Check the box to confirm.",
    "error.departure_refused_last_device":
      "This PC is the last device in your space: the space has to be deleted. Nothing has been erased.",
    "error.departure_refused_update": "Update Tomi to be able to disconnect. Nothing has been erased.",
    "error.departure_refused_relay_too_old":
      "Your Tomi server is too old to delete the space. Update it, then try again. Nothing has been erased.",
    "error.departure_refused_unexpected": "Your Tomi server refused the request. Nothing has been erased.",
    "error.empty_draft": "There is nothing to send.",
    "error.file_too_large": "{name} is over 25 MB.",
    "error.image": "Unreadable image.",
    "error.image_too_large": "This image is too large to be shown here. Open it with “Open”.",
    "error.code_too_large": "This file is too large to be shown (2 MB maximum).",
    "error.code_binary": "This file isn't text: it can't be shown here.",
    "error.internal": "An internal error occurred.",
    "error.invalid_url": "This link is not a valid http(s) address.",
    "error.io": "File access error.",
    "error.item_too_large": "The send is too big (100 MB and 32 files at most).",
    "error.not_found": "Item not found (it may have expired).",
    "error.not_paired": "This PC is not paired with your Tomi space yet.",
    // Pairing (spec/pairing-v1.md §11.5). No message holds the code or any byte of a frame.
    "error.pairing_failed": "The pairing did not complete.",
    "error.pairing_expired": "Code expired. Generate a new one.",
    "error.pairing_taken": "This code has already been used. Generate a new code on the other device.",
    "error.pairing_cancelled": "The pairing was canceled on the other device.",
    "error.pairing_unsupported": "Update Tomi on both devices to pair them.",
    "error.pairing_code_invalid": "This code is not valid. It looks like 7-crayon-renard.",
    "error.pairing_link_invalid": "This link is not a Tomi pairing code.",
    "error.pairing_busy": "A pairing is already open on your space. Try again in a moment.",
    "error.pairing_in_progress": "A pairing is already in progress on this PC.",
    "error.pairing_too_many_devices": "Your space has reached its maximum number of devices.",
    "error.pairing_epoch_conflict": "The keys of your space have just changed. Start again.",
    "error.pairing_device_exists":
      "This device is already part of your space, or has been removed from it: reinstall Tomi on it.",
    "error.pairing_token_invalid": "Installation key incorrect or already used.",
    "error.pairing_space_exists":
      "This key has already been used to create a space. Join it from a device that is part of it.",
    "error.pairing_rate_limited": "Too many pairing attempts in a short time. Try again in a few minutes.",
    "error.pairing_already_paired": "This PC is already part of a space.",
    // The relay (spec/pairing-v1.md §4.4, §10): it comes from a link, a QR code or the "Relay" field.
    "error.pairing_relay_not_configured": "Enter the relay address",
    "error.pairing_relay_invalid":
      "This relay address is not valid. It looks like https://relay.example.org (https required).",
    "error.pairing_relay_mismatch":
      "The link and the “Relay” field give two different relays. Keep the one from the link or clear the field.",
    "error.pairing_setup_link_invalid": "This link is not a Tomi space creation link.",
    "error.pairing_name_invalid": "The name of this PC is not valid (1 to 64 characters).",
    "error.pairing_no_session": "No pairing in progress.",
    "error.pairing_not_ready": "Not yet: wait for the verification code to be shown.",
    "error.not_secret": "This item is not hidden.",
    "error.auth_required": "Verify your identity with Windows Hello first. Nothing has been changed.",
    "error.not_consumable": "This item cannot be consumed this way. Nothing has been changed.",
    "error.nothing_to_copy": "Nothing to copy.",
    "error.nothing_to_open": "Nothing to open.",
    "error.nothing_to_save": "No file to save.",
    "error.secret_no_save":
      "Hidden content cannot be saved to a folder: it would stay there unencrypted. Use “Open” or “Copy”.",
    "error.after_read_with_files":
      "“After reading” is not possible with files: choose another duration.",
    "error.payload_too_large": "The content is too long: a send cannot exceed 1 MB of text and metadata (files have their own limit).",
    "error.offline": "Offline. The send will resume when the network is back.",
    "error.open": "Couldn't open this item.",
    "error.rejected": "The operation was refused.",
    "error.revoke_self": "This PC cannot revoke itself. Remove it from another device.",
    "error.sendto": "Couldn't create the “Send to” shortcut.",
    "error.storage": "Tomi's local storage cannot be accessed.",
    "error.title_too_long": "The title is too long.",
    "error.unknown_device": "This device is no longer part of your space.",
    "error.unreadable": "This item is unreadable.",
    "error.unsupported": "Not supported on this system.",
    "shortcut.altgr": "Ctrl+Alt is AltGr on many keyboards: choose another combination.",
    "shortcut.conflict": "{shortcut} is already used by another application.",
    "shortcut.invalid": "Invalid combination. Example: Ctrl+Shift+Space.",
    "shortcut.needs_modifier": "Add Ctrl, Alt or Shift to the combination.",
    "deeplink.empty": "The Tomi link contains nothing to send.",
    "deeplink.invalid_item": "Invalid Tomi link.",
    "deeplink.invalid_url": "The Tomi link contains an invalid address.",
    "deeplink.pair_invalid": "This link is not a Tomi pairing code.",
    "deeplink.secret": "Hidden content cannot be passed through a link.",
    "deeplink.too_long": "The Tomi link is too long.",
    "deeplink.unknown": "Tomi link not recognized.",
  } as Record<string, string>,
  unknownError: "An error occurred.",
  truncatedPrefill: "The content received was too long and was cut.",
  // Sizes and short delays (`formatBytes`, `formatRemaining`).
  units: {
    bytes: "{n} B",
    kilobytes: "{n} KB",
    megabytes: "{n} MB",
    minutesSeconds: "{m} min {s} s",
    seconds: "{s} s",
  },
  // Time left before an item expires (`formatRemainingTime`).
  duration: {
    minutes: { one: "{n} minute", other: "{n} minutes" },
    hours: { one: "{n} hour", other: "{n} hours" },
    days: { one: "{n} day", other: "{n} days" },
  },
};
