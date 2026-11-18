// Every string shown in the webview. Rust-side strings (tray, toasts) live in strings.rs.
// Tutoiement everywhere; "Masqué" / "Afficher" (never "Secret" / "Révéler").

import type {
  AppError,
  ContentKind,
  ExpiryChoice,
  ItemFilter,
  Rejection,
  RenderMode,
  TransferPhase,
} from "../api";

export const fr = {
  appName: "Tomi",
  empty: {
    all: "Rien pour l'instant. Écris, colle ou dépose quelque chose ci-dessus pour l'envoyer à tes appareils.",
    receiveOnly: "Rien pour l'instant.",
    filtered: "Aucun élément ne correspond.",
    // Android's text says "Glisse une carte de Récents vers la gauche" : there is no swipe on a PC.
    archives: "Aucun élément archivé. Choisis « Archiver » dans le menu d'un élément de Récents pour le ranger ici.",
    search: "Aucun résultat. Les contenus masqués ne sont cherchés que par appareil, jamais par titre ni par contenu.",
  },
  searchResults: {
    one: "{n} résultat",
    other: "{n} résultats",
  },
  // Next to the counter, only when a search has a hidden item among its results.
  searchMaskedNote: "les Masqués sont cherchés sans leur titre ni leur contenu",
  sections: { pinned: "Épinglés", recent: "Récents", archives: "Archives" },
  tabs: { history: "Historique", settings: "Réglages", devices: "Appareils", onboarding: "Bienvenue" },
  sectionsLabel: "Sections",
  filtersLabel: "Filtres",
  filters: {
    all: "Tous",
    links: "Liens",
    text: "Texte",
    files: "Fichiers",
    secrets: "Masqués",
    archives: "Archives",
  } satisfies Record<ItemFilter, string>,
  kinds: {
    link: "Lien",
    text: "Texte",
    richText: "Texte riche",
    image: "Image",
    file: "Fichier",
    files: "Fichiers",
  } satisfies Record<ContentKind, string>,
  // Two modes only, like Android: "Aperçu" (styled) and "Markdown" (the literal source). "Markdown"
  // exists only for an item that has a Markdown representation. HTML is never shown as source.
  renderModes: { preview: "Aperçu", source: "Markdown" } satisfies Record<RenderMode, string>,
  renderModesLabel: "Affichage du texte",
  expiry: {
    never: "Jamais",
    afterFirstRead: "Après lecture",
    hour1: "1 h",
    day1: "1 j",
    day7: "7 j",
    day30: "30 j",
  } satisfies Record<ExpiryChoice, string>,
  composer: {
    label: "Envoyer",
    placeholder: "Écris, colle un lien ou un texte, dépose des fichiers…",
    placeholderSecret: "Valeur à masquer…",
    title: "Titre (facultatif)",
    titleSecret: "Titre masqué (facultatif)",
    url: "Lien",
    secret: "Masquer le contenu",
    secretOff: "Aperçu caché partout jusqu'à « Afficher »",
    secretHint:
      "Chiffré, masqué à l'affichage (titre compris), effacé du presse-papiers après 30 s.",
    showInput: "Voir la saisie",
    hideInput: "Cacher la saisie",
    removeField: "Retirer",
    removeFile: "Retirer {name}",
    addFiles: "Joindre un fichier",
    expires: "Expiration",
    afterReadHint: "« Après lecture » : effacé partout dès la première ouverture.",
    afterReadNoFiles: "« Après lecture » n'est pas proposé avec des fichiers : ils seraient effacés avant d'avoir pu être ouverts.",
    send: "Envoyer",
    drop: "Dépose pour ajouter",
    looksSecret: "Ça ressemble à un mot de passe. Le masquer ?",
    markSecret: "Masquer",
    sourceFormat: "Format détecté",
    to: "Vers {devices}",
    prefillKept: "Le lien ou le titre reçu n'a pas été ajouté : le champ était déjà rempli.",
  },  job: {
    pending: "Envoi dans",
    cancel: "Annuler",
    sending: "Envoi en cours…",
    done: "Envoyé",
    queued: "Enregistré, il partira dès le retour du réseau",
    cancelled: "Envoi annulé",
    failed: "Échec de l'envoi",
    retry: "Réessayer",
    dismiss: "Abandonner",
  },
  card: {
    copy: "Copier",
    copyLink: "Copier le lien",
    copied: "Copié",
    copiedSecret: "Copié · effacé dans 30 s",
    copiedAndGone: "Copié. Ce contenu est maintenant effacé de tous tes appareils.",
    open: "Ouvrir",
    save: "Enregistrer",
    saved: "Enregistré dans {dir}",
    showFolder: "Afficher le dossier",
    reveal: "Afficher",
    hide: "Masquer",
    details: "Détails",
    readMore: "Lire la suite",
    more: "Plus d'actions",
    delete: "Supprimer",
    deleteLocal: "Supprimer de cet appareil",
    deleteEverywhere: "Supprimer partout",
    deletedLocal: "Supprimé de cet appareil",
    deletedEverywhere: "Suppression demandée sur tous les appareils",
    secret: "Masqué",
    pin: "Épingler",
    unpin: "Désépingler",
    pinned: "Épinglé",
    // Archive (spec/archive-contract.md): same words as Android (strings.xml).
    archive: "Archiver",
    unarchive: "Désarchiver",
    archived: "Archivé",
    archivedToast: "Archivé",
    unarchivedToast: "Désarchivé",
    undo: "Annuler",
    undoKey: "Ctrl+Z",
    archiveFailed: "Impossible de modifier l'archive. Réessaie.",
    viewImage: "Voir l'image",
    viewCode: "Ouvrir le fichier",
    fileHidden: "1 fichier masqué",
    filesHidden: "{n} fichiers masqués",
    // "N autres fichiers ›" (button to the detail), then the same as a caption when "Lire la suite" takes its place.
    moreFiles: { one: "{n} autre fichier", other: "{n} autres fichiers" },
    moreFilesCaption: { one: "+ {n} autre fichier", other: "+ {n} autres fichiers" },
    // Count and total size above the attachments ("4 fichiers · 4,9 Mo"); from 2 attachments on.
    summaryFiles: "{n} fichiers · {size}",
    summaryImages: "{n} images · {size}",
    fileGone: "indisponible",
    characters: { one: "{n} caractère", other: "{n} caractères" },
    revealedHint: "Se masque dès que tu quittes la fenêtre.",
    unreadable:
      "Cet élément est illisible pour le moment. Il est conservé : une synchronisation ou une mise à jour peut le rendre lisible.",
    unreadableUpdate:
      "Mise à jour nécessaire pour lire cet élément. Il est conservé et s'affichera après la mise à jour de Tomi.",
    unreadableShort: "Illisible",
    thisPc: "Ce PC",
    from: "{device}",
    toDevices: "vers tes appareils",
    expiresIn: "expire dans {when}",
    expired: "expiré",
    afterRead: "disparaît après la première lecture",
    dragOut: "Glisser le fichier hors de la fenêtre",
    new: "Non lu",
    yesterday: "Hier",
    // Delivery status of a sent item (summary of the per-device states, like Android).
    status: {
      queued: "En attente d'envoi",
      sent: "Envoyé",
      failed: "Échec de l'envoi",
      deliveredAll: "Livré à tous",
      deliveredSome: "Livré à {delivered} sur {total}",
      expired: "Livré à {delivered} · expiré avant livraison à {devices}",
    },
    deleteTitle: "Supprimer partout ?",
    deleteMessage:
      "L'élément sera effacé sur tous tes appareils et sur le relais. Les appareils hors ligne l'effaceront à leur reconnexion. Cette action est définitive.",
    deleteSecretMessage:
      "Ce contenu masqué sera effacé sur tous tes appareils et sur le relais. Un appareil qui l'a déjà affiché a pu en garder une copie. Cette action est définitive.",
  },
  detail: {
    title: "Détail",
    back: "Retour",
    notFound: "Élément introuvable. Il a peut-être expiré ou été supprimé.",
    maskedHint: "Le contenu reste masqué tant que tu n'appuies pas sur « Afficher ».",
    actions: "Actions de l'élément",
    info: "Informations",
    deleteMenu: "Supprimer…",
    deleteTitle: "Supprimer cet élément ?",
    deleteMessage:
      "Supprimer de cet appareil ne touche pas tes autres appareils. Supprimer partout efface l'élément sur tous tes appareils et sur le relais ; les appareils hors ligne l'effaceront à leur reconnexion. C'est définitif.",
    deleteSecretMessage:
      "Supprimer de cet appareil ne touche pas tes autres appareils. Supprimer partout efface ce contenu masqué sur tous tes appareils et sur le relais ; un appareil qui l'a déjà affiché a pu en garder une copie. C'est définitif.",
    // Shown in the tooltips and next to "Retour".
    keys: { back: "Échap", copy: "Ctrl+C", open: "Ctrl+O", save: "Ctrl+S", delete: "Suppr" },
    meta: {
      type: "Type",
      sentAt: "Date d'envoi",
      expiry: "Expiration",
      pinned: "Épinglé",
      status: "Statut",
      yes: "Oui",
      no: "Non",
      never: "Jamais",
      afterRead: "Après lecture",
      received: "Reçu",
    },
  },
  viewer: {
    close: "Fermer",
    previous: "Image précédente",
    next: "Image suivante",
    position: "{n}/{total}",
    description: "Image {n} sur {total}",
    zoomIn: "Zoom avant",
    zoomOut: "Zoom arrière",
    zoomReset: "Taille réelle",
    fit: "Ajuster à la fenêtre",
    loadFailed: "Impossible d'afficher cette image.",
    tooLarge: "Cette image est trop volumineuse pour être affichée ici. Ouvre-la avec « Ouvrir ».",
    loading: "Chargement de l'image…",
    hint: "Molette pour zoomer, glisser pour déplacer, flèches pour changer d'image.",
  },
  code: {
    title: "Fichier",
    search: "Rechercher dans le fichier",
    searchPlaceholder: "Rechercher dans le fichier",
    previous: "Occurrence précédente",
    next: "Occurrence suivante",
    copyFile: "Copier le fichier",
    copied: "Fichier copié",
    wrap: "Retour à la ligne",
    close: "Fermer",
    tooLarge: "Ce fichier est trop volumineux pour être affiché (2 Mo maximum).",
    binary: "Ce fichier n'est pas du texte : impossible de l'afficher ici.",
    unavailable: "Impossible d'afficher ce fichier pour l'instant.",
    noColoursLanguage: "Pas de coloration pour ce langage.",
    noColoursSize: "Coloration indisponible : fichier trop long (50 Ko ou 2 000 lignes maximum).",
    position: "{n}/{total}",
    lines: { one: "{n} ligne", other: "{n} lignes" },
    loading: "Chargement du fichier…",
  },  status: {
    notPaired: "Ce PC n'est pas encore appairé à ton espace Tomi. Tu pourras envoyer et recevoir dès qu'il le sera.",
    revoked: "Ce PC a été retiré de ton espace. Rien ne peut plus y être envoyé ni lu.",
    updateRequired: "Ton espace utilise une version de Tomi plus récente que celle-ci. Mets Tomi à jour pour continuer.",
    blocked: "Ton espace est bloqué pour le moment. Réessaie plus tard.",
    notFound: "Ton serveur Tomi ne trouve plus ton espace. Rien n'a été effacé sur ce PC.",
    storage: "Le stockage local de Tomi est inaccessible. Tes données n'ont pas été touchées.",
    storageCodes: {
      wrap_key_missing:
        "La clé qui protège les données (créée par Windows) est introuvable : elles restent intactes mais illisibles.",
      wrap_key_unreadable:
        "Windows ne peut pas ouvrir la clé qui protège les données : elles viennent d'un autre compte ou d'un autre PC.",
    } as Record<string, string>,
    seeDevices: "Appareils",
  },
  popover: {
    pin: "Garder ouvert",
    unpin: "Ne plus garder ouvert",
    history: "Historique complet",
    settings: "Réglages",
    search: "Rechercher",
  },
  settings: {
    title: "Réglages",
    shortcut: "Raccourci global",
    shortcutHelp: "Ouvre Tomi depuis n'importe où. Exemple : Ctrl+Shift+Space.",
    shortcutApply: "Appliquer",
    shortcutSaved: "Raccourci enregistré : {shortcut}",
    shortcutNotRegistered: "Le raccourci n'est pas actif (déjà utilisé par une autre application ?).",
    autostart: "Lancer Tomi au démarrage de Windows",
    notifications: "Notifications à la réception",
    theme: "Thème",
    themeHelp: "Suit le thème de Windows, ou reste clair ou sombre.",
    themes: { system: "Système", light: "Clair", dark: "Sombre" },
    downloadDir: "Dossier d'enregistrement",
    downloadDirDefault: "Téléchargements\\Tomi",
    downloadDirIsDefault: "Par défaut",
    choose: "Choisir…",
    reset: "Par défaut",
    about: "Tomi {version}",
    quit: "Quitter Tomi",
  },
  // Leaving the space, deleting it, erasing without the server's proof. Same wording as Android
  // (strings.xml), "téléphone" becoming "PC".
  departure: {
    sectionTitle: "Mon espace",
    sectionHelp:
      "Se déconnecter retire ce PC de ton espace et efface tout ce qu'il contient. Tes autres appareils gardent leur copie.",
    unavailable: "La déconnexion n'est pas encore disponible dans cette version de Tomi.",
    cancel: "Annuler",
    retry: "Réessayer",
    // Se déconnecter
    leave: "Se déconnecter",
    leaveTitle: "Se déconnecter de ton espace ?",
    leaveMessage:
      "Ce PC va quitter ton espace. Tout ce qu'il contient sera effacé : éléments, secrets, fichiers et clés, y compris ce qui n'est pas encore envoyé. Tes autres appareils gardent leur copie. Pour revenir, il faudra ajouter de nouveau ce PC depuis un autre appareil.",
    leaveInProgress: "Déconnexion en cours…",
    leaveErrorUpdate: "Mets Tomi à jour pour pouvoir te déconnecter. Rien n'a été effacé.",
    leaveErrorUnexpected: "Ton serveur Tomi a refusé la déconnexion. Rien n'a été effacé.",
    // Supprimer l'espace (the last device)
    deleteTitle: "Supprimer ton espace ?",
    deleteMessage:
      "Tu es le seul appareil. Se déconnecter supprime ton espace et tout son contenu, définitivement.",
    deleteDetail:
      "Ton serveur Tomi efface l'espace, puis ce PC efface tout ce qu'il contient : éléments, secrets, fichiers et clés. Personne ne pourra les récupérer.",
    deleteConfirm: "Supprimer l'espace",
    deleteInProgress: "Suppression en cours…",
    deleteErrorRelayTooOld:
      "Ton serveur Tomi est trop ancien pour supprimer l'espace. Mets-le à jour, puis réessaie. Rien n'a été effacé.",
    deleteErrorUpdate: "Mets Tomi à jour pour pouvoir supprimer ton espace. Rien n'a été effacé.",
    deleteErrorUnexpected:
      "Ton serveur Tomi a refusé la suppression de l'espace. Rien n'a été effacé.",
    // The banner of a departure that waits
    pendingLeaveTitle: "Déconnexion en attente",
    pendingDeleteTitle: "Suppression de l'espace en attente",
    pendingChecking: "Tomi vérifie auprès de ton serveur. Rien n'a été effacé.",
    pendingNetwork:
      "Ton serveur Tomi n'a pas encore confirmé. Rien n'a été effacé. Tomi réessaie automatiquement, au plus une fois par minute, et quand tu reviens sur cette fenêtre ou que le réseau revient.",
    pendingServer: "Ton serveur Tomi a un problème passager. Rien n'a été effacé. Tomi réessaie.",
    pendingNotSent:
      "Ta demande n'a pas abouti : ce PC fait toujours partie de ton espace. Rien n'a été effacé.",
    pendingNotFound:
      "Ton serveur Tomi ne trouve plus ton espace, ou ne reconnaît plus ce PC. Rien n'a été effacé.",
    wiping: "Effacement de ce PC…",
    cancelledLeave:
      "Déconnexion annulée. Si ton serveur l'avait déjà reçue, ce PC apparaîtra comme retiré de l'espace ; ce qu'il contient reste lisible.",
    cancelledDelete:
      "Suppression annulée. Si ton serveur l'avait déjà faite, ton espace n'existe plus que sur ce PC.",
    failed: "L'effacement n'a pas abouti. Rien n'a été effacé. Réessaie.",
    // States without a departure
    notFoundTitle: "Espace introuvable",
    notFoundMessage:
      "Ton serveur Tomi ne trouve plus ton espace. S'il a été réinitialisé, tu peux effacer ce PC. Rien n'a été effacé sur ce PC.",
    revokedTitle: "Retiré de l'espace",
    revokedMessage:
      "Ce PC a été retiré de ton espace. Ce qu'il contient reste lisible ici. Te déconnecter l'efface.",
    // Effacer quand même
    eraseAnyway: "Effacer quand même",
    eraseTitle: "Effacer ce PC sans confirmation du serveur ?",
    eraseMessage:
      "Ton serveur Tomi ne peut pas confirmer que ton espace est supprimé. L'espace n'est peut-être pas supprimé sur ton serveur ; s'il existe encore, ce PC n'y aura plus accès.",
    eraseDetail:
      "Tout ce que ce PC contient sera effacé définitivement : éléments, secrets, fichiers et clés.",
    eraseCheckbox: "J'ai compris : c'est définitif.",
    eraseConfirm: "Effacer ce PC",
    eraseInProgress: "Effacement en cours…",
    eraseChecking: "Tomi vérifie auprès de ton serveur…",
    eraseNotAllowed: "Tomi doit d'abord vérifier auprès de ton serveur. Réessaie dans un instant.",
    // Windows Hello (the prompt itself is drawn by Windows)
    helloNote: "Une vérification Windows Hello te sera demandée.",
    authenticating: "Vérification Windows Hello en cours…",
    // Said to screen readers when the confirm button of an irreversible dialog is armed (2 s).
    ready: "Tu peux maintenant confirmer.",
    // "Annuler" pressed while a request is on its way: it goes on to its end and erases nothing.
    cancellingTitle: "Annulation en cours",
    cancellingText:
      "Tomi attend la fin de la demande déjà partie vers ton serveur. Rien ne sera effacé.",
  },
  devices: {
    title: "Appareils",
    thisDevice: "Cet appareil",
    unverified: "non vérifié",
    lastSeen: "vu {when}",
    never: "jamais vu",
    // Tomi only knows that the device has not shown up, not that it is uninstalled.
    inactive: "Inactif depuis {duration}. Tu ne l'utilises plus ? Retire-le.",
    inactiveDuration: { one: "{n} jour", other: "{n} jours" },
    empty: "Aucun autre appareil.",
    platforms: { windows: "Windows", android: "Android", other: "Autre" },
    notPaired: "Ce PC n'est pas encore appairé : aucun appareil à afficher.",
    // Rename and revoke (same wording as Android)
    more: "Actions pour {device}",
    rename: "Renommer",
    renameLabel: "Nom de l'appareil",
    renameHint: "C'est le nom que tes autres appareils verront (le relais aussi : n'y mets rien de secret).",
    renameSave: "Enregistrer",
    renameSaving: "Enregistrement…",
    renamed: "Appareil renommé.",
    revoke: "Révoquer",
    revokeTitle: "Révoquer « {device} » ?",
    revokeMessage:
      "Cet appareil sera retiré de ton espace et la clé de l'espace sera renouvelée : il ne pourra plus lire les nouveaux éléments. Windows Hello te demandera de confirmer.",
    revoking: "Révocation en cours…",
    revoked: "« {device} » a été révoqué.",
    // Nothing waits for the network here (unlike a send). A timeout after the relay accepted the
    // call is also reported as offline, so the text does not claim that nothing happened.
    offline: "Hors ligne : l'opération n'a peut-être pas abouti. Réessaie quand le réseau est revenu.",
  },
  // Pairing (spec/pairing-v1.md). The code, its link and the verification code are shown
  // and never typed back, copied or logged.
  pairing: {
    // A PC that is not in a space
    create: "Créer mon espace",
    createHint: "Pas d'autre appareil Tomi ? Crée ton espace avec le jeton d'installation.",
    relayUnknown: "Le relais ne répond pas pour l'instant. Vérifie ta connexion, Tomi réessaie automatiquement.",
    back: "Retour",
    deviceName: "Nom de ce PC",
    deviceNameHint: "C'est le nom que tes autres appareils verront.",
    pending: "Un appairage interrompu est en cours de vérification…",
    resumedProvisioned: "L'appairage interrompu est terminé : ce PC fait partie de ton espace.",
    resumedDiscarded:
      "L'appairage interrompu n'a pas abouti : l'autre appareil n'avait pas encore ajouté ce PC. Recommence avec un nouveau code.",
    ok: "OK",

    // First device
    createTitle: "Créer ton espace",
    createIntro:
      "Colle le jeton d'installation ou le lien de création. Il s'affiche une seule fois, quand la personne qui gère le relais le crée (commande {command} sur le serveur). S'il est perdu, elle en crée un autre.",
    tokenLabel: "Jeton d'installation ou lien de création",
    tokenHint: "Il ne sert qu'à cette création et n'est pas gardé. Un lien de création donne aussi le relais.",
    showInput: "Voir la saisie",
    hideInput: "Cacher la saisie",
    createDo: "Créer l'espace",
    creating: "Création de l'espace…",
    created: "Ton espace est créé. Ce PC en est le premier appareil.",

    // New device
    joinTitle: "Rejoindre ton espace",
    joinIntro:
      "Sur ton autre appareil, ouvre Tomi › Appareils › Ajouter un appareil, puis saisis le code court ou colle le lien.",
    codeLabel: "Code d'appairage",
    codePlaceholder: "Par exemple 7-crayon-renard",
    codeHint: "Un code ou un lien. Les majuscules et les accents n'ont pas d'importance. Tab complète le mot proposé.",
    linkReceived: "Lien d'appairage reçu",
    linkReceivedHint: "Rien n'est fait automatiquement. Appuie sur « Rejoindre » pour continuer.",
    linkIgnored: "Ce PC fait déjà partie d'un espace : le lien d'appairage reçu est ignoré.",
    // The relay: read from a link or typed, never trusted by itself (shown in full, confirmed with the code)
    relayLabel: "Relais",
    relayPlaceholder: "https://relay.example.org",
    relayHint:
      "L'adresse du relais de ton espace. Elle est reprise du lien ou du QR code quand il en contient une.",
    relayHintRequired:
      "Ce PC n'a pas encore de relais : indique son adresse, ou colle un lien d'appairage qui la contient.",
    relayOther: "Autre relais",
    relayFromLink: "Relais lu dans le lien : vérifie qu'il est bien le tien.",
    relayUsed: "Relais : {relay}",
    relayNone: "Aucun relais : indique son adresse.",
    relayVerify: "Vérifie que c'est bien ton relais avant de continuer.",
    relayOnScreen: "Relais : {relay}",
    setupLinkReceived: "Lien de création d'espace reçu",
    setupLinkReceivedHint:
      "Rien n'est fait automatiquement. Vérifie le relais, puis appuie sur « Créer l'espace ».",
    setupLinkPasted: "Ce texte est un lien ou un jeton de création d'espace, pas un code d'appairage.",
    setupLinkUse: "Créer mon espace avec",
    joinDo: "Rejoindre",
    joining: "Connexion à l'autre appareil…",
    waitingJoin: "En attente de l'autre appareil…",
    working: "Appairage en cours…",

    // Inviting a device
    addDevice: "Ajouter un appareil",
    inviteTitle: "Ajouter un appareil",
    inviteIntro:
      "Sur le nouvel appareil, ouvre Tomi › Rejoindre, puis scanne le QR code ou saisis le code court.",
    preparing: "Préparation du code…",
    qrLabel: "QR code d'appairage",
    shortCode: "Code court",
    expiresIn: "Valable encore {time}",
    expired: "Ce code expire.",
    renewed: "L'appairage n'a pas abouti. Un nouveau code est prêt.",
    cancel: "Annuler",

    // Comparing the codes
    verifyTitle: "Vérifie le code",
    verifyJoin: "Vérifie que le même code s'affiche sur {device}.",
    verifyInvite:
      "Vérifie que {device} affiche le même code. S'il affiche une erreur ou un autre code, refuse.",
    verifyHint: "Si tu attends trop longtemps, l'appairage expire : tu recommenceras avec un nouveau code.",
    codeDescription: "Code de vérification {code}",
    spaceSizeOne: "Ton espace compte 1 appareil.",
    spaceSizeMany: "Ton espace compte {n} appareils.",
    same: "Les codes sont identiques",
    different: "Ils sont différents",
    add: "Ajouter {device}",
    refuse: "Refuser",
    historyTitle: "Récupérer l'historique ?",
    historySize: "{items} éléments, {size}",
    historyYes: "Oui, récupérer l'historique",
    historyYesHint: "Tout ce qui est dans ton espace arrive ici, contenus masqués et fichiers compris.",
    historyNo: "Non, partir d'un fil vide",
    historyNoHint: "Seuls les prochains envois arriveront ici.",

    // After the decision
    addingTitle: "Ajout de {device}",
    joiningTitle: "Connexion à ton espace",
    provisioningInvite: "Si ce n'est pas déjà fait, confirme aussi le code sur {device}.",
    provisioningJoin: "En attente de la confirmation sur {device}…",
    doneInvite: "{device} fait partie de ton espace.",
    doneJoin: "Ce PC fait partie de ton espace.",
    historyPreparingInvite:
      "L'historique est en préparation pour {device}. Tu peux suivre l'envoi plus bas.",
    historyPreparingJoin: "L'historique de ton espace arrive dans quelques instants.",
    finish: "Terminé",
    newCode: "Nouveau code",
    close: "Fermer",
    offline: "Réseau indisponible. L'appairage reprend dès qu'il revient.",
    // The same failure is told differently to the device that typed the code and to the one that
    // showed it (spec/pairing-v1.md §11.5). The one that showed it offers a new code itself.
    failedJoin: "Code incorrect. Génère un nouveau code sur l'autre appareil.",
    failedInvite: "L'appairage n'a pas abouti.",
  },
  transfer: {
    title: "Historique",
    offer: "{device} te propose son historique.",
    offerBody:
      "Tout ce qui est dans son fil arrive ici, contenus masqués et fichiers compris. Tu peux répondre non : tu partiras d'un fil vide.",
    accept: "Oui, récupérer l'historique",
    decline: "Non",
    send: "Transférer l'historique…",
    sendAsk:
      "Envoyer tout l'historique à {device} ? Contenus masqués et fichiers compris, scellé pour cet appareil seulement.",
    sendYes: "Oui, envoyer",
    sendNo: "Non",
    cancel: "Annuler",
    toDevice: "vers {device}",
    fromDevice: "depuis {device}",
    progress: "{done} / {total} éléments",
    files: "{done} / {total} fichiers",
    phases: {
      offered: "Proposé",
      running: "En cours",
      done: "Terminé",
      failed: "Échec",
      cancelled: "Annulé",
      declined: "Refusé",
    } satisfies Record<TransferPhase, string>,
  },
  onboarding: {
    title: "Bienvenue dans Tomi",
    intro: "Envoie un texte, un lien, un mot de passe ou un fichier à tes autres appareils, et reçois de même.",
    steps: [
      "Clique sur l'icône de la zone de notification, ou utilise le raccourci {shortcut}.",
      "Colle ou dépose ce que tu veux envoyer, puis valide. Tu as quelques secondes pour annuler.",
      "Clic droit sur un fichier > Envoyer vers > Tomi fonctionne aussi.",
    ],
    done: "C'est parti",
  },
  common: { close: "Fermer", loading: "Chargement…", cancel: "Annuler", ok: "OK" },
  time: {
    now: "à l'instant",
    minutes: "il y a {n} min",
    hours: "il y a {n} h",
    days: "il y a {n} j",
    inMinutes: "dans {n} min",
    inHours: "dans {n} h",
    inDays: "dans {n} j",
  },
  rejected: {
    tooLarge: "{name} : plus de 25 Mo, refusé.",
    itemTotalExceeded: "{name} : l'envoi dépasserait 100 Mo.",
    notAFile: "{name} : ce n'est pas un fichier.",
    tooMany: "{name} : trop de fichiers dans un seul envoi (32 au maximum).",
  } satisfies Record<Rejection["reason"], string>,
  errors: {
    "error.attachment_missing": "Un fichier joint est introuvable.",
    "error.autostart": "Impossible de modifier le démarrage automatique.",
    "error.blocked": "Ce PC ne peut plus utiliser ton espace (retiré, ou Tomi à mettre à jour).",
    "error.clipboard": "Le presse-papiers est indisponible. Réessaie.",
    "error.clock": "L'horloge de ce PC semble fausse : l'épingle ou l'archive n'a pas pu être enregistrée.",
    "error.core": "Erreur interne du cœur Tomi.",
    "error.devices_changed": "Les appareils de ton espace ont changé pendant l'opération. Réessaie.",
    "error.device_name_invalid":
      "Ce nom n'est pas valable : il doit faire de 1 à 64 caractères, sans caractère de contrôle.",
    "error.download_dir": "Dossier d'enregistrement introuvable.",
    // Windows Hello (departure, revoking a device, showing or copying hidden content): asked in Rust, never bypassed
    "error.auth_cancelled": "Vérification annulée. Rien n'a été modifié.",
    "error.auth_failed": "La vérification Windows n'a pas abouti (Windows Hello ou mot de passe du compte). Rien n'a été modifié. Réessaie.",
    "error.auth_busy": "Windows Hello est occupé. Réessaie dans un instant. Rien n'a été modifié.",
    "error.strong_auth_unavailable":
      "Aucune vérification n'est possible sur ce PC : Windows Hello est indisponible et le mot de passe de ton compte Windows ne peut pas être demandé. Rien n'a été modifié.",
    "error.strong_auth_no_password":
      "Windows Hello est indisponible et ton compte Windows n'a pas de mot de passe, donc rien ne peut être vérifié. Définis un mot de passe pour ton compte Windows (ou configure Windows Hello), puis réessaie. Rien n'a été modifié.",
    "error.strong_auth_too_many":
      "Trop d'essais avec le mot de passe de ton compte Windows : attends une minute, puis réessaie. Rien n'a été modifié.",
    "error.strong_auth_locked":
      "Ton compte Windows est verrouillé (trop de mots de passe erronés). Déverrouille-le ou attends, puis réessaie. Rien n'a été modifié.",
    "error.departure_unavailable": "La déconnexion n'est pas encore disponible dans cette version de Tomi.",
    "error.departure_state": "Cette fenêtre n'est plus à jour. Rien n'a été effacé.",
    "error.departure_busy": "Une opération est déjà en cours.",
    "error.departure_not_armed": "Patiente deux secondes avant de confirmer.",
    "error.erase_not_allowed": "Tomi doit d'abord vérifier auprès de ton serveur. Réessaie dans un instant.",
    "error.erase_not_checked": "Coche la case pour confirmer.",
    "error.departure_refused_last_device":
      "Ce PC est le dernier appareil de ton espace : il faut supprimer l'espace. Rien n'a été effacé.",
    "error.departure_refused_update": "Mets Tomi à jour pour pouvoir te déconnecter. Rien n'a été effacé.",
    "error.departure_refused_relay_too_old":
      "Ton serveur Tomi est trop ancien pour supprimer l'espace. Mets-le à jour, puis réessaie. Rien n'a été effacé.",
    "error.departure_refused_unexpected": "Ton serveur Tomi a refusé la demande. Rien n'a été effacé.",
    "error.empty_draft": "Il n'y a rien à envoyer.",
    "error.file_too_large": "{name} dépasse 25 Mo.",
    "error.image": "Image illisible.",
    "error.image_too_large": "Cette image est trop volumineuse pour être affichée ici. Ouvre-la avec « Ouvrir ».",
    "error.code_too_large": "Ce fichier est trop volumineux pour être affiché (2 Mo maximum).",
    "error.code_binary": "Ce fichier n'est pas du texte : impossible de l'afficher ici.",
    "error.internal": "Une erreur interne est survenue.",
    "error.invalid_url": "Ce lien n'est pas une adresse http(s) valide.",
    "error.io": "Erreur d'accès aux fichiers.",
    "error.item_too_large": "L'envoi est trop gros (100 Mo et 32 fichiers au maximum).",
    "error.not_found": "Élément introuvable (peut-être expiré).",
    "error.not_paired": "Ce PC n'est pas encore appairé à ton espace Tomi.",
    // Pairing (spec/pairing-v1.md §11.5). No message holds the code or any byte of a frame.
    "error.pairing_failed": "L'appairage n'a pas abouti.",
    "error.pairing_expired": "Code expiré. Génères-en un nouveau.",
    "error.pairing_taken": "Ce code a déjà servi. Génère un nouveau code sur l'autre appareil.",
    "error.pairing_cancelled": "L'appairage a été annulé sur l'autre appareil.",
    "error.pairing_unsupported": "Mets Tomi à jour sur les deux appareils pour les appairer.",
    "error.pairing_code_invalid": "Ce code n'est pas valide. Il ressemble à 7-crayon-renard.",
    "error.pairing_link_invalid": "Ce lien n'est pas un code d'appairage Tomi.",
    "error.pairing_busy": "Un appairage est déjà ouvert sur ton espace. Réessaie dans un instant.",
    "error.pairing_in_progress": "Un appairage est déjà en cours sur ce PC.",
    "error.pairing_too_many_devices": "Ton espace a atteint son nombre maximal d'appareils.",
    "error.pairing_epoch_conflict": "Les clés de ton espace viennent de changer. Recommence.",
    "error.pairing_device_exists":
      "Cet appareil fait déjà partie de ton espace, ou en a été retiré : réinstalle Tomi dessus.",
    "error.pairing_token_invalid": "Jeton d'installation incorrect ou déjà utilisé.",
    "error.pairing_space_exists":
      "Ce jeton a déjà servi à créer un espace. Rejoins-le depuis un appareil qui en fait partie.",
    "error.pairing_rate_limited": "Trop d'essais d'appairage en peu de temps. Réessaie dans quelques minutes.",
    "error.pairing_already_paired": "Ce PC fait déjà partie d'un espace.",
    // The relay (spec/pairing-v1.md §4.4, §10): it comes from a link, a QR code or the "Relais" field.
    "error.pairing_relay_not_configured": "Indique l'adresse du relais",
    "error.pairing_relay_invalid":
      "Cette adresse de relais n'est pas valable. Elle ressemble à https://relay.example.org (https obligatoire).",
    "error.pairing_relay_mismatch":
      "Le lien et le champ « Relais » indiquent deux relais différents. Garde celui du lien ou efface le champ.",
    "error.pairing_setup_link_invalid": "Ce lien n'est pas un lien de création d'espace Tomi.",
    "error.pairing_name_invalid": "Le nom de ce PC n'est pas valable (1 à 64 caractères).",
    "error.pairing_no_session": "Aucun appairage en cours.",
    "error.pairing_not_ready": "Pas encore : attends que le code de vérification s'affiche.",
    "error.not_secret": "Cet élément n'est pas masqué.",
    "error.auth_required": "Vérifie d'abord ton identité avec Windows Hello. Rien n'a été modifié.",
    "error.not_consumable": "Cet élément ne se consomme pas ainsi. Rien n'a été modifié.",
    "error.nothing_to_copy": "Rien à copier.",
    "error.nothing_to_open": "Rien à ouvrir.",
    "error.nothing_to_save": "Aucun fichier à enregistrer.",
    "error.secret_no_save":
      "Un contenu masqué ne s'enregistre pas dans un dossier : il y resterait en clair. Utilise « Ouvrir » ou « Copier ».",
    "error.after_read_with_files":
      "« Après lecture » n'est pas possible avec des fichiers : choisis une autre durée.",
    "error.payload_too_large": "Le contenu est trop long : un envoi ne peut pas dépasser 1 Mo de texte et de métadonnées (les fichiers ont leur propre limite).",
    "error.offline": "Hors ligne. L'envoi reprendra au retour du réseau.",
    "error.open": "Impossible d'ouvrir cet élément.",
    "error.rejected": "L'opération a été refusée.",
    "error.revoke_self": "Ce PC ne peut pas se révoquer lui-même. Retire-le depuis un autre appareil.",
    "error.sendto": "Impossible de créer le raccourci « Envoyer vers ».",
    "error.storage": "Le stockage local de Tomi est inaccessible.",
    "error.title_too_long": "Le titre est trop long.",
    "error.unknown_device": "Cet appareil ne fait plus partie de ton espace.",
    "error.unreadable": "Cet élément est illisible.",
    "error.unsupported": "Non pris en charge sur ce système.",
    "shortcut.altgr": "Ctrl+Alt est AltGr sur beaucoup de claviers : choisis une autre combinaison.",
    "shortcut.conflict": "{shortcut} est déjà utilisé par une autre application.",
    "shortcut.invalid": "Combinaison invalide. Exemple : Ctrl+Shift+Space.",
    "shortcut.needs_modifier": "Ajoute Ctrl, Alt ou Maj à la combinaison.",
    "deeplink.empty": "Le lien Tomi ne contient rien à envoyer.",
    "deeplink.invalid_item": "Lien Tomi invalide.",
    "deeplink.invalid_url": "Le lien Tomi contient une adresse invalide.",
    "deeplink.pair_invalid": "Ce lien n'est pas un code d'appairage Tomi.",
    "deeplink.secret": "Un contenu masqué ne peut pas être transmis par un lien.",
    "deeplink.too_long": "Le lien Tomi est trop long.",
    "deeplink.unknown": "Lien Tomi non reconnu.",
  } as Record<string, string>,
  unknownError: "Une erreur est survenue.",
  truncatedPrefill: "Le contenu reçu était trop long et a été coupé.",
};

/** `format("de {device}", { device: "Pixel" })` */
export function format(template: string, params: Record<string, unknown> = {}): string {
  return template.replace(/\{(\w+)\}/g, (_, k: string) => String(params[k] ?? ""));
}

export function errorMessage(err: AppError | string): string {
  const code = typeof err === "string" ? err : err.code;
  const params = typeof err === "string" ? {} : err.params;
  const template = fr.errors[code];
  return template ? format(template, params) : fr.unknownError;
}

/**
 * The message of the error that ended a pairing. "Code incorrect" is told differently to the
 * device that typed the code and to the one that showed it (spec/pairing-v1.md §11.5).
 */
export function pairingErrorMessage(err: AppError | null, role: "invite" | "join"): string {
  if (!err) return "";
  if (err.code === "error.pairing_failed") {
    return role === "join" ? fr.pairing.failedJoin : fr.pairing.failedInvite;
  }
  return errorMessage(err);
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} o`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(n < 10 * 1024 ? 1 : 0)} Ko`;
  return `${(n / 1024 / 1024).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} Mo`;
}

/** A short delay, `1 min 42 s` or `42 s`, rounded up to the second. */
export function formatRemaining(ms: number): string {
  const total = Math.max(0, Math.ceil(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return minutes > 0 ? `${minutes} min ${seconds} s` : `${seconds} s`;
}

/** Relative time, past or future, in French. */
export function relativeTime(ts: number, now = Date.now()): string {
  const diff = ts - now;
  const abs = Math.abs(diff);
  const min = Math.round(abs / 60_000);
  const t = fr.time;
  if (diff <= 0) {
    if (min < 1) return t.now;
    if (min < 60) return format(t.minutes, { n: min });
    const h = Math.round(min / 60);
    if (h < 24) return format(t.hours, { n: h });
    return format(t.days, { n: Math.round(h / 24) });
  }
  if (min < 60) return format(t.inMinutes, { n: Math.max(min, 1) });
  const h = Math.round(min / 60);
  if (h < 48) return format(t.inHours, { n: h });
  return format(t.inDays, { n: Math.round(h / 24) });
}

/** French plural: 0 and 1 are singular. `plural({ one: "{n} jour", other: "{n} jours" }, 3)` */
export function plural(forms: { one: string; other: string }, n: number): string {
  return format(n < 2 ? forms.one : forms.other, { n });
}

const TIME = new Intl.DateTimeFormat("fr-FR", { hour: "2-digit", minute: "2-digit" });
const DAY = new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short" });
const DAY_YEAR = new Intl.DateTimeFormat("fr-FR", { day: "numeric", month: "short", year: "numeric" });
const DATE_TIME = new Intl.DateTimeFormat("fr-FR", {
  day: "numeric",
  month: "long",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
});

/** `14:02` today, `Hier`, `7 nov.` this year, `7 nov. 2025` before (charter 7, like Android). */
export function formatTimestamp(ts: number, now = Date.now()): string {
  const d = new Date(ts);
  const n = new Date(now);
  const startOf = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const days = Math.round((startOf(n) - startOf(d)) / 86_400_000);
  if (days <= 0) return TIME.format(d);
  if (days === 1) return fr.card.yesterday;
  return (d.getFullYear() === n.getFullYear() ? DAY : DAY_YEAR).format(d);
}

/** `7 novembre 2026 à 14:02`, for the detail screen. */
export function formatDateTime(ts: number): string {
  return DATE_TIME.format(new Date(ts));
}

const DATE_TIME_SECONDS = new Intl.DateTimeFormat("fr-FR", {
  day: "numeric",
  month: "long",
  year: "numeric",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
});

/** `7 novembre 2026 à 14:02:37`: the exact send date, for the detail screen. */
export function formatPreciseDateTime(ts: number): string {
  return DATE_TIME_SECONDS.format(new Date(ts));
}

/** `3 jours`, `2 heures`, `1 minute`: time left before an item expires. */
export function formatRemainingTime(ms: number): string {
  const minutes = Math.max(1, Math.round(ms / 60_000));
  if (minutes < 60) return `${minutes} minute${minutes > 1 ? "s" : ""}`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `${hours} heure${hours > 1 ? "s" : ""}`;
  const days = Math.round(hours / 24);
  return `${days} jours`;
}
