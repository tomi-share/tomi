//! User-visible strings produced on the Rust side (tray, tooltip, toasts), in French.
//! Everything shown in the webview lives in `src/lib/i18n/fr.ts`.

pub const APP_NAME: &str = "Tomi";

pub const TRAY_OPEN: &str = "Ouvrir";
pub const TRAY_HISTORY: &str = "Historique";
pub const TRAY_SETTINGS: &str = "Réglages";
pub const TRAY_QUIT: &str = "Quitter";

pub const TOAST_COPY: &str = "Copier";
pub const TOAST_OPEN: &str = "Ouvrir";

pub fn tooltip(unread: usize, sending: bool) -> String {
    let mut s = APP_NAME.to_string();
    if sending {
        s.push_str(" · envoi en cours…");
    }
    match unread {
        0 => {}
        1 => s.push_str(" · 1 non lu"),
        n => s.push_str(&format!(" · {n} non lus")),
    }
    s
}

pub fn toast_title_link() -> &'static str {
    "Nouveau lien"
}
pub fn toast_title_text() -> &'static str {
    "Nouveau texte"
}
pub fn toast_title_image() -> &'static str {
    "Nouvelle image"
}
pub fn toast_title_file() -> &'static str {
    "Nouveau fichier"
}
pub fn toast_title_files() -> &'static str {
    "Nouveaux fichiers"
}
pub fn toast_title_secret() -> &'static str {
    "Contenu masqué reçu"
}
/// An item that cannot be read: the title names only the sender (as on Android).
pub fn toast_title_unreadable(device: &str) -> String {
    format!("Élément de {device}")
}
/// ... and the body says why, with the advice that fits (nothing of the item is known).
pub fn toast_body_unreadable_update() -> &'static str {
    "Illisible pour le moment : mise à jour de Tomi nécessaire"
}
pub fn toast_body_unreadable_other() -> &'static str {
    "Illisible pour le moment"
}
pub fn toast_title_offer() -> &'static str {
    "Historique proposé"
}
/// Another device just joined the space: a security notice, so the user can react if it is not
/// theirs (the Devices tab revokes it).
pub fn toast_title_new_device() -> &'static str {
    "Nouvel appareil ajouté"
}
pub fn toast_body_new_device(device: &str) -> String {
    format!("{device} a rejoint ton espace Tomi. Ce n'est pas toi ? Révoque-le.")
}
pub fn toast_from(device: &str) -> String {
    format!("de {device}")
}
pub fn toast_group_title(n: usize) -> String {
    format!("{n} nouveaux éléments")
}
pub fn toast_group_body() -> &'static str {
    "Ouvre Tomi pour les consulter"
}

// Verification prompts (see `strong_auth.rs`): the sentence Windows shows above the PIN,
// fingerprint or face check of Windows Hello. Same wording as the Android biometric prompts.
// When Hello is unavailable the password of the Windows account is asked instead
// (`account_auth.rs`) with its own sentence (`account_prompt`): a cancelled Hello never opens
// that fallback, so the Hello sentences do not mention it.
pub const HELLO_DELETE_SPACE: &str =
    "Supprimer ton espace Tomi. Action irréversible : une vérification Windows Hello est nécessaire.";
pub const HELLO_ERASE: &str =
    "Effacer ce PC sans confirmation du serveur. Action irréversible : une vérification Windows Hello est nécessaire.";
pub const HELLO_REVOKE: &str =
    "Révoquer un appareil de ton espace Tomi. Les clés de l'espace sont renouvelées : une vérification Windows Hello est nécessaire.";
pub const HELLO_REVEAL: &str =
    "Afficher, copier ou ouvrir un contenu masqué. Une vérification Windows Hello est nécessaire (valable 2 minutes).";
pub const HELLO_ADD_DEVICE: &str =
    "Ajouter un appareil à ton espace Tomi. Il recevra la clé de l'espace : une vérification Windows Hello est nécessaire.";

pub const ACCOUNT_DELETE_SPACE: &str =
    "Supprimer ton espace Tomi. Action irréversible : saisis le mot de passe de ton compte Windows pour confirmer.";
pub const ACCOUNT_ERASE: &str =
    "Effacer ce PC sans confirmation du serveur. Action irréversible : saisis le mot de passe de ton compte Windows pour confirmer.";
pub const ACCOUNT_REVOKE: &str =
    "Révoquer un appareil de ton espace Tomi. Les clés de l'espace sont renouvelées : saisis le mot de passe de ton compte Windows pour confirmer.";
pub const ACCOUNT_REVEAL: &str =
    "Afficher, copier ou ouvrir un contenu masqué : saisis le mot de passe de ton compte Windows (valable 2 minutes).";
pub const ACCOUNT_ADD_DEVICE: &str =
    "Ajouter un appareil à ton espace Tomi. Il recevra la clé de l'espace : saisis le mot de passe de ton compte Windows pour confirmer.";
pub const ACCOUNT_GENERIC: &str = "Saisis le mot de passe de ton compte Windows pour confirmer.";

/// The sentence of the account-password prompt that goes with a Hello sentence.
pub fn account_prompt(hello: &str) -> &'static str {
    match hello {
        HELLO_DELETE_SPACE => ACCOUNT_DELETE_SPACE,
        HELLO_ERASE => ACCOUNT_ERASE,
        HELLO_REVOKE => ACCOUNT_REVOKE,
        HELLO_REVEAL => ACCOUNT_REVEAL,
        HELLO_ADD_DEVICE => ACCOUNT_ADD_DEVICE,
        _ => ACCOUNT_GENERIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hello_sentence_has_its_account_sentence_and_neither_mentions_the_other() {
        for (hello, account) in [
            (HELLO_DELETE_SPACE, ACCOUNT_DELETE_SPACE),
            (HELLO_ERASE, ACCOUNT_ERASE),
            (HELLO_REVOKE, ACCOUNT_REVOKE),
            (HELLO_REVEAL, ACCOUNT_REVEAL),
            (HELLO_ADD_DEVICE, ACCOUNT_ADD_DEVICE),
        ] {
            assert_eq!(account_prompt(hello), account);
            assert!(!hello.contains("mot de passe"));
            assert!(!account.contains("Hello"));
        }
        assert_eq!(account_prompt("autre"), ACCOUNT_GENERIC);
    }
}
