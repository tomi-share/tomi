# Actions dans l'admin web du relais : décision sécurité / ergonomie

Statut : **décidé le 24/11/2026, porté dans `apps/relay`** (voir « Portage » en fin de document pour les écarts ; le propriétaire a délégué les arbitrages, voir « Décisions prises » en fin de
document). Remplace le §16 de `DECISIONS-UI.md` (« aucune action destructrice »). Sources lues : `apps/relay/src/admin_web.rs`, `webauthn.rs`, `admin.rs`
(`revoke`, `reset`, `quota`, `issue_invitation`), `revoke.rs`, `state.rs` (`delete_space`), `spec/api-v1.md`
§1, §5.4, §7.2, §7.6, §7.9.6, §8.7, §8.7.1, `apps/relay/README.md`, `infra/forge/enable-admin-web.md`.

## Principe retenu

**Une passkey avec user verification, demandée à chaque action qui détruit ou qui crée un droit**, sous la forme
d'un *step-up* lié à l'action : défi à usage unique, valable 2 min, attaché à la session et à l'empreinte exacte
de l'action, vérifié par le serveur qui recalcule l'action et revérifie ses conditions dans la transaction
d'écriture. Les actions réversibles sur-le-champ (quotas, annuler des invitations, fermer les sessions) restent à
la session seule, avec confirmation dans la page et journal. Le reste de la protection vient de garde-fous
sans nouveau secret :

1. **Plusieurs passkeys** (5 au plus), enrôlées une à une par le code CLI actuel, dont une **clé liée à
   l'appareil** recommandée (clé matérielle, drapeau BE = 0). C'est la vraie réponse à la passkey synchronisée
   compromise et à la perte de l'unique passkey, et elle remplace avantageusement le TOTP.
2. **Plafonds de débit par relais** sur les actions destructrices (pas par adresse) : au-delà, le CLI.
3. **Corbeille côté relais** pour la suppression d'espace depuis le web : effet immédiat et identique pour les
   appareils (`4004`), dossier chiffré conservé 7 jours, restauration **CLI seulement**. Ce n'est pas un délai.
4. **Journal d'audit persistant** des actions destructrices, avec l'identifiant court de la cible.
5. **CLI seul** pour tout ce qui touche à l'accès au panneau lui-même (passkeys, chemin, réinitialisation) et
   pour la restauration.

Ainsi : une session volée lit des métadonnées et change au pire un quota (réversible) ; elle ne retire rien, ne
supprime rien et n'obtient aucun jeton d'invitation.

## Modèle de menace

| Menace | Ce que l'attaquant obtient avec ces décisions | Ce qui le limite | Résiduel assumé |
|---|---|---|---|
| **Cookie de session volé** (logiciel malveillant dans le navigateur, poste partagé) | Lectures (métadonnées), quotas, annulation des invitations, fermeture des sessions | Pas de step-up possible sans passkey ; session 30 min d'inactivité / 8 h ; « Fermer toutes les sessions » ; journal | Déni de service réversible par quota |
| **XSS** dans la page | Ce que permet la session, et **la substitution de l'action** au prochain step-up que tu lances : l'authentificateur n'affiche jamais l'action, seulement « se connecter à relais.example » | CSP `script-src 'self'` sans inline, zéro code tiers, DOM en `textContent` (XSS très improbable) ; une action par geste ; plafonds ; corbeille ; journal | Aucune solution 100 % WebAuthn n'y échappe ; seul un second canal le ferait (voir appareil couplé, écarté) |
| **CSRF** | Rien | `SameSite=Strict`, `Origin` exact, `X-Tomi-Admin: 1` (existant) ; en plus, le défi lié à la session | — |
| **Rejeu** d'une assertion | Rien | Défi à usage unique, 2 min, lié à (session, empreinte de l'action), comparaison en temps constant ; compteur de signature si l'authentificateur en a un | — |
| **Passkey synchronisée compromise** (compte iCloud / Google / gestionnaire pris) | Tout le web, dans la limite des plafonds | Politique optionnelle « suppression d'espace = clé liée à l'appareil » ; plafonds ; corbeille ; journal persistant ; `admin web reset` coupe tout | Retraits d'appareils dans le plafond (réversibles par recouplage) |
| **Appareil déverrouillé volé** (session ouverte) | Comme un cookie volé ; plus, si le voleur connaît le code de l'appareil, ce qu'offre la passkey de cet appareil | UV exigé (biométrie ou code) ; 30 min d'inactivité ; clé matérielle pour la suppression si la politique est active | Comme dans l'app Tomi (`api-v1.md` §1, D39) |
| **Relais compromis** (machine prise) | Tout, comme aujourd'hui, sans le panneau | Hors périmètre ; la passkey n'ajoute aucun secret récupérable côté relais (seulement des clés publiques) | La corbeille garde plus longtemps des octets chiffrés que root pouvait déjà copier |
| **Erreur de cible** (mauvais espace, mauvais appareil) | — | Récapitulatif fourni **par le serveur** (nom, plateforme, dernière activité, 8 caractères) ; saisie des 8 premiers caractères pour supprimer un espace ; une cible par action ; corbeille | Retrait du mauvais appareil : recouplage |
| **Perte de l'unique passkey** | — | Seconde passkey enrôlée ; sinon CLI : `admin web reset` puis `admin web enroll` (inchangé) | Sans seconde clé : il faut SSH |

## Ce que fait vraiment chaque action (d'après le code)

- **Retirer un appareil** (`revoke_device`, chemin `via: "admin"`) : la clé de signature passe dans
  `revoked_sign_pks` pour toujours, certificat, présence et push effacés, sessions révoquées, WebSocket fermée en
  `4003`, canaux d'appairage fermés, transferts d'historique purgés, `rotation_required = 1`. Un membre restant
  fait la rotation tout seul (core, sous 0-30 s, §7.6). L'appareil retiré passe REVOKED et **garde ses données
  lisibles** (§7.9.6 : une révocation n'efface rien). Le dernier membre est refusé. **Réversible** au prix d'un
  recouplage (nouvelles clés, historique complet transféré) depuis un autre appareil membre. Gravité moyenne :
  perte de service, aucune perte de données.
- **Supprimer un espace** (`delete_space`, comme `admin reset`) : désinscription, WebSockets en `4004`,
  dossier (base et blobs), routage, quota et invitations consommées effacés. Pas de reçu : les appareils passent
  NOT_FOUND et **n'effacent rien**, et aucun parcours ne republie un espace depuis un appareil (vérifié, A7).
  Aujourd'hui **irréversible côté relais**, hors restauration complète d'une archive `infra/backup.sh` (qui ramène tout
  `routing.db`, donc aussi les autres espaces). Gravité haute.
- **Clé d'espace « à renouveler »** : le relais **ne peut pas** renouveler la clé (elle n'existe que sur les
  appareils, la rotation est un `KeyRotation` signé par un membre). « Valider » en effaçant `rotation_required`
  laisserait à l'appareil retiré la clé courante : **interdit**. Le drapeau qui reste levé veut dire qu'aucun
  membre ne s'est connecté depuis le retrait ; le panneau l'explique et ne propose aucune action.
- **Créer une invitation / remplacer les invitations** (`issue_invitation`) : fabrique un jeton qui donne le
  droit de **créer un espace** sur ton relais (disque, hébergement d'un inconnu) ; « remplacer » invalide aussi
  les jetons en attente. Le jeton est dans la réponse : une session volée le récupère. Gravité moyenne.
- **Quota d'un espace / quota par défaut** (`set_quota_override`, `set_default_quota`) : rien n'est effacé ; un
  espace au-dessus du plafond voit ses écritures refusées (`507 relay_full`) jusqu'au retour sous la limite.
  Réversible immédiatement. Gravité faible (déni de service visible).
- **Rotation du chemin** (`--rotate-path`) : ferme toutes les sessions, l'ancienne adresse devient un 404. Depuis
  le web, elle ne protège de rien que « Fermer toutes les sessions » ne couvre, et elle t'enferme dehors si tu
  n'as pas noté la nouvelle adresse.
- **Enrôlement d'une passkey** : donne l'accès au panneau pour toujours. Depuis le web, même derrière un
  step-up, une passkey compromise en ajouterait une autre à elle : persistance après ta reprise en main.

## Matrice action → protection → parcours

| Action | Protection | Parcours en une ligne |
|---|---|---|
| Retirer un ou plusieurs appareils d'un espace | Step-up ; plafond 10 / h ; refus du dernier membre (renvoie vers Supprimer l'espace) | Cocher, « Retirer » → feuille : nom, plateforme, dernière activité, « devra être recouplé » → biométrie → résultat « rotation en attente » |
| Supprimer un espace | Step-up + saisie des 8 premiers caractères + plafond 2 / 24 h + corbeille ; clé liée à l'appareil si la politique est active ; **pas** d'invitation créée (comme `DELETE /v1/space`) | « Supprimer l'espace » → récapitulatif (appareils, taille, « restaurable 7 jours par le serveur ») → saisir `794e87f1` → biométrie → retour à la liste avec bandeau |
| Créer une invitation | Step-up ; plafond 5 / h | « Créer » → biométrie → jeton en haut, une fois (inchangé) |
| Remplacer les invitations en attente (`admin invite --rotate`) | Step-up (crée un jeton) | « Remplacer… » → confirmation « N invitations invalidées » → biométrie → jeton |
| Invalider les invitations en attente (nouveau, et nouveau en CLI : `admin invite --cancel`) | Session + confirmation (ne fait que retirer un droit) | « Invalider… » → confirmation → fait |
| Quota d'un espace (nouveau sur le web) | Session + confirmation si la valeur est sous l'usage actuel | Champ + Enregistrer ; si l'espace serait bloqué : « Les envois de 794e87f1 seront refusés » → Confirmer |
| Quota par défaut | Session (inchangé) + même confirmation, avec la liste des espaces qui seraient bloqués | Inchangé, plus la confirmation conditionnelle |
| Fermer toutes les sessions (nouveau) | Session (action de protection) | ⋮ → « Fermer toutes les sessions » → retour à la connexion |
| État de la clé d'espace | Aucune action (lecture) | Pastille « Clé à renouveler » + phrase d'explication |
| Rotation du chemin, ajout / retrait / réinitialisation de passkey, politique de clé, restauration et purge de la corbeille, désactivation du panneau | **CLI seul** | Le panneau montre l'état (passkeys : type et dernière utilisation ; corbeille : contenu) et la commande à copier |

Plafonds : comptés **par relais** (pas par session ni par adresse : ouvrir une autre session n'en donne pas
d'autres), en mémoire, sur les seules actions **réussies**, en plus du seau `SENSITIVE` existant ; valeurs fixes
dans le code (pas de clé de configuration, qui ne servirait qu'à baisser la garde : le CLI reste le recours),
exposées dans `api/about`. Un redémarrage les remet à zéro, ce qu'une session volée ne sait pas provoquer.
Vérifiés **avant** de fabriquer le défi ; un dépassement répond `429` avec la commande CLI équivalente. **5 step-up refusés dans une session ferment cette session** (jamais la
passkey : sinon un voleur de cookie pourrait t'enfermer dehors).

## Pourquoi à chaque action et pas une fenêtre de ré-authentification

Une fenêtre (« 5 min après la dernière vérification ») rend au cookie volé dans la fenêtre tout le pouvoir
destructeur, ce que la garantie interdit. Le coût du « à chaque action » est faible : les actions destructrices
sont rares, une biométrie prend une seconde, et le retrait de plusieurs appareils d'un même espace passe en **un
seul** step-up (l'empreinte couvre la liste). La connexion elle-même ne vaut pas step-up.

## Alternatives écartées

- **TOTP en plus de la passkey : non.** Le seul gain serait contre une passkey compromise sans le TOTP ; or les
  deux vivent le plus souvent dans le même gestionnaire ou le même compte cloud, ce qui annule le gain. Le TOTP
  exige une graine côté relais (secret récupérable, contraire au relais aveugle), se fait hameçonner, ne se lie
  pas à l'action et coûte une saisie de 6 chiffres à chaque fois. Une clé liée à l'appareil apporte le même gain,
  en mieux (non synchronisable, liée à l'origine), sans secret serveur.
- **Délai ou annulation de 24 h : non** (rejeté). La corbeille n'en est pas un : l'espace disparaît tout de
  suite pour tout le monde, seul le serveur peut le ramener.
- **Approbation par un appareil Tomi couplé : non, réexaminé.** C'est la seule parade à un XSS au moment du
  geste (second canal qui afficherait l'action). Mais l'app tourne le plus souvent sur le téléphone qui porte la
  passkey (même point de défaillance), il faudrait un rôle « administrateur du relais » qui n'existe pas dans le
  protocole (un relais héberge plusieurs espaces : un appareil d'un espace n'a aucune autorité sur le relais), un
  nouveau message et une nouvelle vérification côté relais et dans les trois apps. Coût très au-dessus du gain
  tant que la CSP rend le XSS improbable.
- **Ajouter une passkey depuis le web (avec step-up)** : non, persistance d'un attaquant (voir plus haut).
- **Afficher l'action dans l'authentificateur** (`txAuthSimple`) : extension abandonnée, aucun authentificateur
  courant ne l'affiche.
- **Notification hors bande** (courriel, ntfy, push vers les appareils) : le relais n'a pas de canal vers
  l'administrateur, et pousser vers les appareils d'un espace leur donnerait des métadonnées d'administration.
  À revoir seulement si un tel canal apparaît.
- **Rotation du chemin depuis le web** : remplacée par « Fermer toutes les sessions », qui couvre le besoin
  d'urgence sans risque de s'enfermer dehors.

## Conséquences côté relais (API, stockage, journal)

**Step-up.**
1. `POST api/stepup/options` `{action}` : session valide, `Origin` et en-tête vérifiés ; le serveur valide
   l'action (cible existante, 8 caractères corrects, plafond non atteint), calcule
   `h = SHA-256("tomi-admin-action-v1" || 0x00 || JSON canonique de l'action)` avec des identifiants **complets**
   (jamais un préfixe), tire un `nonce` de 32 octets et prend comme défi `SHA-256(nonce || h)`. Il garde en
   mémoire `{nonce, h, empreinte de session, époque, expiration +2 min}` (cérémonie `StepUp`, à côté de
   `Register` et `Login`) et répond `publicKey` (UV `required`, `allowCredentials` selon la politique) et un
   **récapitulatif construit par le serveur** (ce que la feuille affiche).
2. `POST api/actions` `{action, assertion}` : même session, recalcul de `h`, défi retiré en temps constant
   (usage unique), assertion vérifiée comme à la connexion (origine, `rpIdHash`, UP+UV, compteur), conditions
   revérifiées **dans** la transaction d'écriture (appareil encore membre, pas le dernier, espace encore inscrit),
   exécution par les fonctions partagées de `admin.rs`, journal, réponse avec l'état **après** (rotation en
   attente, invitations restantes, espace en corbeille).
3. Une seule route d'action pour toutes les actions à step-up : `revoke_devices {space_id, device_ids[]}`,
   `delete_space {space_id, confirm}`, `create_invitation {mode}`. `POST api/invitations` exige désormais une
   assertion (la page et le serveur sont dans le même binaire, rien à garder). Routes de session seule :
   `POST api/quota` (inchangée), `POST api/spaces/quota` `{space_id, value}`, `POST api/invitations/cancel`,
   `POST api/sessions/close-all`. Lecture ajoutée : `GET api/about` donne `sessions.others` (sessions vivantes autres
   que celle qui lit, comptées sur la liste en mémoire), pour « Autres sessions ouvertes ».

**Stockage (`routing.db`, migrations additives, `user_version` inchangé).**
- `admin_web_credentials` (nouvelle table, plusieurs lignes) : identifiant, clé publique, compteur,
  `backup_eligible` lu à l'enrôlement (le drapeau BE est signé par l'authentificateur ; il le déclare, sans
  attestation), `created_at`, `last_used_at`, libellé **généré** (« Clé 2 · liée à l'appareil »), jamais un
  texte saisi. Le numéro N ne se réutilise jamais (une clé retirée garde le sien), pour que « clé 2 » dans le
  journal d'audit désigne toujours la même clé ; chaque session retient le numéro de la clé qui l'a ouverte. La première clé reste aussi dans `admin_web_credential` pour qu'un binaire précédent démarre
  encore. L'enrôlement n'est plus « fermé pour toujours » : ses routes répondent 404 sauf code CLI en cours.
- `admin_web_policy` : `critical_requires_device_bound` (booléen, CLI seulement).
- `admin_web_audit` : actions destructrices et accès (connexion, step-up refusé, passkey ajoutée ou retirée,
  chemin changé, corbeille restaurée ou purgée) ; date, mot fixe, identifiant court (8 caractères) de l'espace
  ou de l'appareil, numéro de la passkey ; **jamais** de nom d'appareil, d'adresse, de jeton ni de valeur de
  requête. Borné (500 lignes, 90 jours). Le journal en mémoire actuel reste pour le reste.
- Corbeille : `spaces.trash/<space_id>-<date>/` (renommage, aucun octet copié) et table `space_trash`
  (`space_id`, `deleted_at`, `purge_after`, taille). Purge par la boucle de maintenance ; la corbeille compte
  dans l'espace disque libre (`min_free_disk_bytes`), pas dans `max_relay_bytes`. `admin trash` (liste),
  `admin trash restore [--space <id>]` (refusée si une clé d'appareil de l'espace a fondé un autre espace
  entre-temps, ou si l'identifiant est repris), `admin trash purge [--space <id>] [--confirm <8 caractères>]`
  (syntaxe : A9). La restauration remet le dossier en place **et** reconstruit les lignes de routage
  (`device_id`, clés de signature) depuis la base de l'espace, puisque la suppression les a effacées ; `space_trash`
  garde aussi le quota propre de l'espace pour le rendre. `admin reset` en CLI garde son effacement immédiat.

**Spec (`api-v1.md`, aucun changement de format ni de vecteur).**
- §8.7.1 réécrit : liste des actions et de leur protection, step-up et sa liaison, plusieurs passkeys et
  enrôlement rouvert par code CLI, plafonds, journal d'audit, corbeille de la suppression web. L'exception
  reste bornée et désactivée par défaut.
- §8.7 : `admin invite --cancel`, `admin trash …`, `admin web policy`, `admin web keys` (syntaxe : A9).
- §1 (tableau des menaces) : ligne « session du panneau volée » et ligne « passkey compromise ».
- §11 : nouveaux tests d'acceptation (défi lié à une autre action ou à une autre session refusé, rejeu refusé,
  dernier membre refusé, plafonds, 5 échecs ferment la session, restauration de corbeille avec et sans conflit).
- Clients : rien à changer (`4003` et `4004` gardent leur sens). Reprise après restauration **vérifiée** (A6).
- Aussi à mettre à jour au portage : `apps/relay/README.md` (panneau, modèle de menace),
  `infra/forge/enable-admin-web.md`, `docs/FEATURES.md`, et le §16 de `DECISIONS-UI.md`.

## Décisions prises (24/11/2026)

Le propriétaire a délégué les arbitrages. Les choix d'interface (I1 à I13) sont dans `DECISIONS-UI.md`.

- **A1. Corbeille de 7 jours** pour la suppression web, restauration et purge sur le serveur seulement. Seule
  protection qui couvre à la fois l'erreur de cible et la passkey compromise sans retarder l'action ; le coût
  (du disque pendant 7 jours) se libère par `admin trash purge`. `admin reset` en CLI garde l'effacement immédiat.
- **A2. Politique « suppression d'espace = clé liée à l'appareil »** : proposée, **désactivée par défaut**,
  activable par CLI seulement et refusée tant qu'aucune clé liée à l'appareil n'est enrôlée. Automatique, elle
  t'empêcherait de supprimer en déplacement ; et le drapeau BE n'est qu'une déclaration de l'authentificateur
  (pas d'attestation), à présenter comme tel.
- **A3. Journal d'audit persistant** (500 lignes, 90 jours) pour les seules actions destructrices et d'accès, avec
  l'identifiant court de la cible et le numéro de la clé ; jamais de nom d'appareil, d'adresse, de jeton ni de
  valeur de requête. Sans cible, un journal de suppression ne sert à rien ; l'exception à « rien sur un espace »
  est bornée à 8 caractères d'identifiant, que le panneau affiche déjà.
- **A4. Plafonds** : 2 suppressions d'espace par 24 h, 10 retraits d'appareil par heure, 5 invitations par heure,
  par relais, comptés sur les réussites, valeurs fixes (voir « Plafonds » plus haut). Au-dessus de l'usage d'une
  personne (moins de cinq espaces), assez bas pour qu'une session ou une passkey volée ne vide pas le relais avant
  que le journal et la corbeille servent.
- **A5. `admin invite --rotate` remplace** les invitations en attente par une nouvelle : vérifié dans le code
  (`admin.rs`, `issue_invitation` : `install::clear` puis `install::create`, jeton affiché ; `main.rs` :
  « Replaces every pending invitation by a new one and prints its token » ; `--new` et `--rotate` sont
  incompatibles). Dans `apps/relay/README.md`, la ligne des commandes (« replaces ») a raison ; la phrase
  « `admin invite --rotate` only removes pending invitations » (§ Running) veut dire « ne touche pas aux invitations
  consommées » et sera récrite au portage (« replaces the pending invitations with a new one; consumed ones stay »).
  Maquette corrigée : « Remplacer » = `admin invite --rotate` seul ; « Invalider » n'avait pas d'équivalent, d'où
  `admin invite --cancel` (A9).
- **A6. Reprise des appareils après restauration : vérifiée, elle marche sans rien changer côté client**, à deux
  réserves. Le core ne persiste jamais NOT_FOUND (`api-v1.md` §7.2 ; `crates/core/src/runtime/engine.rs`,
  `ensure_session`) et redemande au relais à chaque appel réseau, au plus une fois par minute : après
  restauration, un appareil reprend avec ses clés et son curseur à sa prochaine action réseau (ouverture de l'app,
  envoi, actualisation). Mais la sonnette, elle, s'est arrêtée au `4004` (`still_member_after_space_reset`
  renvoie faux, §5.4) : un appareil qu'on n'ouvre pas ne revient pas tout seul. D'où : (1) `restore` doit
  reconstruire le routage (`device_id`, clés de signature) que la suppression a effacé (voir « Corbeille » plus
  haut), sans quoi la session répondrait encore `401` ; (2) sa sortie dit « ouvre Tomi sur chaque appareil de
  l'espace ».
- **A7. Aucun parcours ne republie un espace depuis un appareil : vérifié.** `create_space` refuse un appareil déjà
  provisionné (`crates/core/src/runtime/pairing/founder.rs` : « device already provisioned ») et l'app Android
  n'offre à un espace NOT_FOUND que « Effacer quand même » (`DepartureBanner.kt`). Les données restent lisibles sur
  les appareils, mais la corbeille est le **seul** moyen de rendre l'espace : A1 n'est pas un confort. Conséquence
  pour la feuille de suppression : ne pas laisser croire qu'un appareil pourra « recréer » l'espace (la maquette ne
  le dit pas).
- **A8. Clés d'accès** : numéros jamais réutilisés ; `admin web keys remove` ferme les sessions ouvertes avec la
  clé retirée, refuse la dernière clé (c'est `admin web reset`) et la dernière clé liée à l'appareil tant que A2 est
  active. Sans ces règles, le journal d'audit et la politique pourraient mentir ou se contourner.
- **A9. Syntaxe des nouvelles commandes**, calquée sur l'existant (`invite [--new | --rotate]` pour les modes,
  `--space <id>` pour désigner un espace, sous-commandes sous `web` comme `enroll` et `reset`) :
  - `tomi-relay admin invite --cancel` : invalide les invitations en attente sans en créer (exclusive de `--new`
    et `--rotate`).
  - `tomi-relay admin web keys [--json]` : liste les clés (numéro, type, ajout, dernière utilisation).
  - `tomi-relay admin web keys remove <N>` : retire la clé N et ferme les sessions ouvertes avec elle ; refuse la
    dernière clé (c'est `admin web reset`) et la dernière clé liée à l'appareil tant que la politique l'exige.
  - `tomi-relay admin web policy [--space-deletion <any-key | device-key>]` : sans option, affiche la politique ;
    `device-key` est refusé (code 2) si aucune clé liée à l'appareil n'est enrôlée.
  - `tomi-relay admin trash [--json]` : liste la corbeille.
  - `tomi-relay admin trash restore [--space <id>]` et `tomi-relay admin trash purge [--space <id>] [--confirm
    <8 caractères>]` : même règle que les autres commandes (préfixe d'au moins 8 caractères ; sans `--space`, une
    corbeille d'une seule entrée l'utilise, sinon refus et liste) ; `purge` redemande les 8 caractères comme
    `reset`. Pas de « tout vider » d'un coup : avec moins de cinq espaces, il n'apporte rien et serait la commande
    la plus destructrice sans cible.

### Ce qui reste bloquant côté propriétaire

Rien.

## Portage dans `apps/relay` (branche `feat/relay-admin-actions`) : écarts

Porté tel que décidé ; ce qui diffère ou reste à vérifier :

- **Routage à la restauration.** Les tables de routage (appareil, clé de signature, session, canal) ne sont pas
  stockées : le relais les reconstruit des bases d'espace. `restore` rattache donc l'espace à l'index en mémoire
  (`Spaces::restore_from_trash`) au lieu de réécrire des lignes ; le résultat est celui décrit (A6).
- **Corbeille** : dossier `spaces.trash/<space_id>-<secondes unix>/` ; le renommage refusé n'a pas de repli « suppression
  sur place » (la suppression web échoue sans rien supprimer). Un espace restauré redonne son quota propre.
- **Enrôlement** : tant qu'aucune clé n'existe, les routes d'enrôlement restent ouvertes (un code est de toute façon
  exigé) ; dès qu'une clé existe, elles répondent 404 sauf code en cours.
- **Appareils** : `api/devices` gagne `id` (8 premiers caractères du `device_id`) pour désigner la cible d'un retrait.
- **Journal** : `api/journal` ajoute `audit` (persistant) aux `entries` en mémoire ; la page les fusionne. Revue : la
  borne de 500 lignes ne s'applique plus qu'aux lignes ordinaires (les lignes destructrices ne sont purgées qu'au bout
  de 90 jours), les routes de session seule ont la limite de débit des autres routes sensibles, et l'audit nomme la clé
  qui a signé le step-up.
- **Page** : la maquette a été reprise par script (`index.html`, `app.css`) ; les icônes sont celles de la maquette
  (tâche I8 faite à la revue : tracés officiels Material Symbols Outlined, 24 px, graisse 400, dans le sprite de
  `index.html`, licence Apache 2.0 dans `design/icons/LICENSE-material-symbols.txt`). Aucune vérification
  dans un vrai navigateur avec une vraie clé d'accès, ni sur appareil (zone du geste, clavier virtuel).
