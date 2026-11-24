# Panneau d'administration du relais : décisions UI proposées (refonte mobile + desktop)

Maquette : `index.html` (prototype navigable, téléphone sous 1024 px et desktop au-delà, actions comprises),
`etats.html` (planche des états : connexion, chargement, vide, erreur, variantes, et parcours des actions en feuille
basse et en boîte centrée), `maquette.css`, `maquette.js` (démonstration seulement). Statique, données fictives,
aucune requête réseau, aucune dépendance, aucun attribut `style` ni gestionnaire en ligne (CSP du panneau). Ouvrir
`index.html` dans un navigateur ; le thème se change dans le pied de la barre latérale (desktop) ou par ⋮
(téléphone). La clé d'accès est simulée : pendant l'attente, des boutons propres à la maquette choisissent sa
réponse (valide, annulée, refusée, expirée, réseau coupé).
Statut : **proposition**, rien n'est porté dans `apps/relay/src/admin_web/`. Les actions suivent
[DECISION-ACTIONS.md](DECISION-ACTIONS.md), proposition elle aussi.

Vérifié dans le navigateur de prévisualisation à 390 px, 1280 px et 1440 px, en clair et en sombre, servi avec la
CSP du panneau (`style-src 'self'`, `script-src 'self'`) : aucune vue ne défile horizontalement, aucun texte coupé,
toutes les cibles tactiles font au moins 44 px sur téléphone (hors contrôle segmenté du thème : 40 px dans un cadre
de 44). Sur desktop, à la souris, les boutons de second rang descendent à 36 px.

## Constats sur l'existant (Galaxy S23, 412 px)

- Barre d'onglets à 6 entrées coupée à « Quot… », sans indice de défilement ; la page est une seule colonne de
  7 sections, on ne sait jamais où l'on est.
- L'en-tête (session, thème, Se déconnecter) occupe 280 px avant le premier contenu utile.
- Une carte d'espace fait environ 430 px de haut : un libellé au-dessus de chaque valeur, une donnée par ligne,
  deux boutons de 56 px. Deux espaces ne tiennent pas sur un écran.
- Le détail « Appareils » s'ouvre **sous** la liste, loin du bouton qui l'a ouvert, en blocs clé / valeur empilés.
- « 11 événements retenus (n° 0 à 1 794 262 459 586) » : un compteur interne illisible exposé tel quel.
- Quota : 9 lignes d'explication avant le champ, puis trois gros boutons de même poids.
- Tout a le même poids visuel : titres, valeurs, aides et actions se confondent.

## Décisions proposées

1. **Navigation à 5 destinations** : Aperçu, Espaces, Invitations, Journal, Réglages. Barre basse sur téléphone
   (64 px, icône + libellé, pastille sur l'entrée courante), barre latérale de 224 px à partir de 1024 px.
   « Quota par défaut » et « À propos et diagnostic » fusionnent dans **Réglages** pour tenir en 5 sans entrée
   « Plus ». Préférée à l'en-tête compact avec menu : rien n'est tronqué, la position courante reste visible et
   tout est sous le pouce. Ce n'est pas la barre flottante M3 de l'app Android : ici c'est une page web.
2. **Une vue à la fois, pilotée par l'adresse** (`#espaces`, `#espaces/794e87f1`) : le bouton Retour du
   téléphone ferme le détail, un lien vers un espace se partage, l'actualisation garde la vue.
3. **Barre du haut de 56 px** : logo 24 px, titre de la vue, « Actualiser » en toutes lettres, ⋮. Pas d'icône à
   flèches circulaires ni de spinner (charte 6.3) : l'actualisation en cours est un filet linéaire sous la barre.
4. **Session repliée** : ⋮ ouvre une feuille basse (durée restante, heure de mise à jour, thème en contrôle
   segmenté Auto / Clair / Sombre, « Ce que le relais voit », Se déconnecter). Sur desktop, le même bloc est le
   pied de la barre latérale. Seule une session qui se ferme dans 5 min ou moins remonte dans la barre (puce ocre
   « 4 min »). Avec la puce, sur téléphone, le titre de la vue d'ensemble prend son libellé court « Aperçu »
   pour ne pas être coupé.
5. **Vue d'ensemble en un écran** : une carte d'état (« Rien à signaler » ou « 2 points d'attention », version et
   durée de service en dessous), les points d'attention **cliquables** (Voir l'espace, Journal), puis des tuiles :
   stockage en pleine largeur avec jauge, et quatre tuiles en grille 2 x 2 (4 x 1 sur desktop). « Ce que le relais
   voit » est replié sur téléphone, ouvert en colonne de droite sur desktop.
6. **Espaces : un seul `<table>`, mis en cartes par CSS** (requêtes de conteneur sur la largeur de la liste, pas
   de l'écran). Carte : identifiant + copie et pastille d'état en tête, Appareils / Contenu en deux colonnes, jauge
   d'usage avec valeurs, dernière activité et bouton « Appareils » en bas. Environ 207 px par carte : trois par
   écran. Tableau complet à partir de 832 px de liste ; tableau resserré (identifiant, état, appareils,
   utilisation) quand le détail est ouvert à côté ; colonne « Créé » à partir de 1056 px.
7. **Détail d'un espace** : page dédiée sur téléphone (« Retour » remplace le logo, l'identifiant sert de titre),
   panneau de 368 px collé à droite sur desktop, la liste restant visible et la ligne choisie surlignée.
   Contenu : quatre chiffres en grille, jauge, état de la clé, appareils, commandes serveur repliées.
8. **Appareils en lignes denses** de 56 px : icône de plateforme, nom, « Windows · 0.5.1 · il y a 12 min », un
   seul mot d'état (Présent, sinon Actif, sinon Inactif). Le reste (ajouté, notifications, empreinte + copie)
   se déplie au toucher.
9. **Nombres et dates lisibles** : durées relatives (« il y a 5 j », date exacte en infobulle et en sous-ligne du
   détail), tailles en Mio / Gio, pourcentage toujours écrit à côté de la jauge. Le journal d'un espace devient
   « 312 événements · retenus 45 j au plus » : plus de n° de séquence (ils ont la taille d'un horodatage en ms et
   ne servent qu'au diagnostic ; ils restent dans `tomi-relay admin status --space`).
10. **Pastilles courtes, toujours un mot** : Normal, « Quota à 94 % » (nouveau, dès 90 %, déduit des données déjà
    lues), « Clé à renouveler » (au lieu de « Changement de clé en attente », la phrase complète est dans le
    détail), « Inactif » (au lieu de « Aucun appareil actif »). La couleur n'est jamais seule à porter le sens.
11. **Formulaires courts, explication derrière « ? »** (quota, invitations, journal). Quota : valeur courante en
    grand, champ + unité « Mio » + Enregistrer sur une ligne, aperçu ou refus sous le champ pendant la frappe,
    « Sans limite par espace » et « Revenir à la configuration » en actions discrètes (la première confirme).
12. **Invitations** : le jeton s'affiche **en haut de la vue**, juste après la création, dans un cadre appuyé :
    avertissement « Affiché une seule fois », jeton en mono, « Copier le jeton » en action principale, lien
    replié, QR, « Masquer ». Jamais affiché autrement, masqué après 10 min ou en quittant la page (inchangé).
    « Remplacer les invitations en attente » devient une action discrète sous la liste, avec confirmation.
13. **Retours d'action** : carte encre sombre dans les deux thèmes, au-dessus de la navigation (décision Android
    du 11 nov. 2026) ; un succès s'efface seul, une erreur reste jusqu'à « Fermer ». Confirmation en feuille basse
    sous 640 px, boutons en pleine largeur, « Annuler » a le focus.
14. **États** (voir `etats.html`) : vide avec l'action qui en sort, erreur dans sa section avec « Réessayer »,
    premier chargement en gabarits + filet linéaire, écran de connexion sans navigation.
15. **Charte** : jetons de `design/tomi/tokens.json` (clair et sombre), Inter / JetBrains Mono avec replis système,
    rayons 8 et 12, logo en `currentColor` (nuit sur clair, blanc sur sombre), nuit et soleil nulle part ailleurs,
    assemblage horizontal à 28 px, symbole au pixel à 24 px. Tutoiement, phrases courtes, « : » et « ? » avec
    espace insécable.
16. **Actions dans le panneau** (remplace « aucune action destructrice ») : d'après
    [DECISION-ACTIONS.md](DECISION-ACTIONS.md), détaillées dans la section « Actions » ci-dessous. Clé d'accès à
    **chaque** action qui détruit ou crée un droit (retirer des appareils, supprimer un espace, créer ou remplacer
    une invitation), session seule pour le réversible (quotas, invalider les invitations, fermer les sessions).
    Les commandes `tomi-relay admin …` ne sont plus qu'un repli discret « Équivalent en ligne de commande ».
    **Relais aveugle** inchangé : la maquette ne montre que des métadonnées (comptes, tailles, dates, identifiants
    courts, nom d'appareil, empreinte), et rien sur la clé d'un espace, que le relais n'a pas.

## Actions (d'après DECISION-ACTIONS.md)

### Où sont les actions

| Action | Où | Protection | Parcours |
|---|---|---|---|
| Retirer un ou plusieurs appareils | Détail d'un espace : case devant chaque appareil, barre de sélection ; « Retirer de l'espace… » dans la ligne dépliée ; sur desktop, icône au survol de la ligne | Clé d'accès, une seule pour toute la sélection ; plafond 10 / h | Feuille : appareils visés (nom, plateforme, version, activité, identifiant court), conséquences, clé → retour « X retiré », pastille « Clé à renouveler » |
| Supprimer un espace | Détail : carte « Supprimer l'espace », en bas, avant le repli CLI | Saisie des 8 premiers caractères + clé d'accès ; plafond 2 / 24 h ; corbeille 7 j | Feuille : chiffres de l'espace, conséquences, champ, clé → liste des espaces avec bandeau (corbeille, commande de restauration) |
| Créer une invitation | Invitations | Clé d'accès, sans étape de récapitulatif ; plafond 5 / h | « Créer » → clé → jeton en haut de la vue (inchangé) |
| Remplacer les invitations | Invitations, « Remplacer… » | Clé d'accès | Feuille : invitations invalidées, conséquences, clé → jeton |
| Invalider les invitations | Invitations, « Invalider… » | Session + confirmation | Confirmation avec la liste → fait |
| Quota d'un espace | Détail, « Modifier le quota » sous la jauge | Session ; confirmation si l'espace serait bloqué, et pour « Sans limite » | Champ + aperçu (ocre sous l'usage) → Enregistrer |
| Quota par défaut | Réglages (inchangé) | Session ; confirmation avec la liste des espaces bloqués | Inchangé, plus la confirmation conditionnelle |
| Fermer toutes les sessions | ⋮ (téléphone), pied de la barre latérale (desktop), Réglages > Accès | Session (protection) | Confirmation, bouton principal (pas rouge) → connexion |
| Clés d'accès, politique de clé, adresse, réinitialisation | Réglages > Accès au panneau | **Serveur seulement** | Lecture (type, dates, « Cette session ») ; un encadré dit pourquoi, la commande est dans le repli |
| Corbeille | Réglages > Corbeille | **Serveur seulement** pour restaurer et vider | Contenu et échéance ; encadré « pourquoi » ; commandes dans le repli |
| Clé d'espace à renouveler | Détail, ligne d'état | Aucune action | La phrase dit pourquoi il n'y a pas de bouton (le relais n'a pas la clé) |

### Parcours de confirmation

- **Téléphone** : feuille basse, boutons pleine largeur, l'action en haut et « Annuler » dessous, « Annuler » a le
  focus. **Desktop** : boîte centrée de 34 rem, boutons côte à côte, « Annuler » a le focus, Échap ferme (rappel
  « Échap pour annuler » à gauche).
- Le **récapitulatif vient du serveur** (`api/stepup/options`) et reste visible pendant l'attente de la clé :
  l'authentificateur n'affiche que « relais.example », jamais l'action.
- États (planche `etats.html#actions`) : récapitulatif ; saisie des 8 caractères (bouton inactif tant que la
  valeur n'est pas exacte, sans tenir compte de la casse) ; **clé en attente** (filet linéaire, décompte des 2 min,
  seul « Annuler ») ; **annulée sur l'appareil** (ocre, rien n'est compté) ; **refusée par le relais** (rouge,
  « Refus N sur 5 » et cinq traits) ; **délai dépassé** (« Recommencer » demande un défi neuf) ; **le relais n'a
  pas répondu** (issue inconnue : « Actualiser », jamais un faux succès) ; **limite atteinte** (vérifiée avant le
  défi, aucune clé demandée, commande serveur ouverte d'office) ; **cible changée entre-temps** (« Déjà fait
  ailleurs ») ; **session fermée après 5 refus** (seule issue : la connexion, la clé reste valable).
- Succès : retour qui s'efface seul (retrait, invitation, quota), ou bandeau qui reste jusqu'à « Fermer »
  (suppression d'un espace : la corbeille et sa commande de restauration doivent rester lisibles).
- Garde-fous dans la page, avant toute feuille : dernier appareil (case grisée + phrase), tous les appareils
  cochés (refus dans la barre de sélection), aucun raccourci clavier pour une action qui retire, supprime ou
  invite.

### Desktop

Même document que le téléphone (le relais sert une seule page), couche ajoutée à partir de 1024 px : tableau des
espaces plus dense, ligne survolée ou marquée au clavier en surbrillance, copie de l'identifiant au survol, panneau
de détail de 25 rem (28 rem dès 1440 px) avec « Retirer » rapide au survol d'une ligne d'appareil et barre de
sélection collée au bas du panneau. Raccourcis : `G` puis `A` `E` `I` `J` `R` (vues), `J` `K` ou flèches (espace
suivant, précédent ; le détail ouvert suit), `Entrée` (ouvrir), `Échap` (fermer la boîte, puis le détail), `R`
(actualiser), `C` (copier l'identifiant ouvert), `?` (aide). Rappelés au survol de la navigation, au-dessus du
tableau et dans le pied de la barre latérale.

### Choix pris là où DECISION-ACTIONS.md laissait la main (option recommandée)

1. **Corbeille de 7 jours**, restauration et purge en CLI seulement : appliquée (bandeau, carte Corbeille).
2. **Politique « suppression = clé liée à l'appareil »** : proposée, **désactivée par défaut** ; Réglages affiche
   « N'importe laquelle · réglage du serveur », la planche montre la feuille quand elle est active.
3. **Journal d'audit persistant avec identifiants courts** : appliqué ; les lignes gardées portent « clé N · gardé
   90 j », jamais un nom d'appareil.
4. **Plafonds** : 2 suppressions d'espace par 24 h, 10 retraits d'appareil par heure, 5 invitations par heure, par
   relais ; affichés sous chaque bouton concerné et dans « Limites de la configuration ».

### Choix d'interface pris dans la maquette

- **Un seul document** responsive plutôt qu'un `desktop.html` : le relais sert une page, la maquette doit pouvoir
  se porter telle quelle.
- **« Invalider »** au lieu de « Annuler les invitations » : sinon la confirmation aurait deux boutons
  « Annuler ».
- **Annulée ≠ refusée** : une annulation sur l'appareil n'envoie rien au relais et n'est pas comptée ; seule une
  assertion rejetée compte dans les 5 refus.
- **Cases toujours visibles** devant les appareils ; l'icône de plateforme leur cède la place (la plateforme est
  déjà écrite dans la sous-ligne).
- **« Sans limite » pour un espace** demande confirmation, comme pour le quota par défaut.
- **« Fermer toutes les sessions »** est en bouton principal ou discret, jamais rouge : c'est une protection.
- **Syntaxe des nouvelles commandes CLI** inventée pour la maquette (`admin web keys [--remove N]`,
  `admin web policy --delete-needs-device-key on`, `admin trash`, `admin trash restore <id>`,
  `admin trash purge [<id>]`) : à fixer au portage.

### Points à trancher

1. **Un document ou deux** pour le desktop (proposé : un seul, voir plus haut).
2. **« Retirer » rapide au survol** d'une ligne d'appareil sur desktop : utile pour un retrait isolé, la feuille de
   confirmation reste. À garder ou à réserver à la ligne dépliée.
3. **Compteur de refus visible** (« Refus 3 sur 5 ») : proposé, pour qu'une faute de clé ne ferme pas la session
   par surprise ; il renseigne aussi un voleur de cookie, qui ne gagne rien à le savoir.
4. **« Sessions ouvertes : 2 »** dans Réglages > Accès : demande au serveur de compter ses sessions (donnée
   nouvelle). À garder ou à retirer.
5. **`admin invite --rotate`** : `apps/relay/README.md` dit à un endroit qu'il « remplace », à un autre qu'il
   « retire seulement » les invitations en attente. La maquette suit la seconde lecture (« Invalider » =
   `--rotate`, « Remplacer » = `--rotate` puis `--new`). À clarifier dans le README.
6. **Décompte des 2 min visible** pendant l'attente : proposé ; on peut n'afficher que l'état expiré.

### Changements à faire pour porter les actions (non faits)

Côté serveur, tout est dans DECISION-ACTIONS.md (« Conséquences côté relais ») : routes `api/stepup/options` et
`api/actions`, routes de session seule (`api/spaces/quota`, `api/invitations/cancel`, `api/sessions/close-all`),
tables `admin_web_credentials`, `admin_web_policy`, `admin_web_audit`, `space_trash`. **Ces actions changent donc
l'API du panneau**, contrairement à la refonte d'interface ci-dessous. Côté page :

- `index.html` : `dialog#stepup` (récapitulatif, saisie, attente, message, repli CLI), `ul.recap-list` dans
  `#confirm-dialog`, `#d-selbar`, `#sq-form`, `#d-danger`, `#spaces-banner`, cartes `#access` et `#trash`,
  `dialog#shortcuts` ; nouvelles icônes `i-key`, `i-delete`, `i-person-remove`, `i-lock`, `i-info`, `i-keyboard`.
- `app.css` : sections « actions », « confirmations » et « desktop : densité, survols, raccourcis » de
  `maquette.css`.
- `app.js` : machine d'états de la feuille (`data-state`) branchée sur les vraies réponses (`NotAllowedError` →
  annulée, `401`/assertion rejetée → refusée, `429` → limite, `409` → cible changée, réseau → issue inconnue,
  session fermée → connexion), `AbortController` sur Échap et « Annuler », plafonds lus dans `api/about`. Le DOM se
  construit en `textContent` (la maquette utilise `innerHTML` par commodité).

## Ancien → nouveau, section par section

| Ancien (une seule page) | Nouveau |
|---|---|
| En-tête : titre, « Session : encore 30 min », Thème (liste), Se déconnecter | Barre de 56 px ; session, thème et déconnexion dans la feuille ⋮ (téléphone) ou le pied de la barre latérale (desktop) |
| Onglets d'ancres (6, coupés) | Barre basse à 5 destinations / barre latérale ; une vue à la fois |
| Vue d'ensemble : « Mis à jour à », Actualiser, « Rien à signaler », 6 tuiles empilées, aide dépliable | Vue **Aperçu** : carte d'état + alertes cliquables, tuile stockage, 4 tuiles en grille ; « Actualiser » dans la barre ; version et durée de service dans la carte d'état |
| Espaces : cartes clé / valeur, bouton « Appareils » | Vue **Espaces** : cartes compactes (téléphone) ou tableau triable (desktop), tri par liste déroulante sur téléphone |
| Appareils de l'espace : section sous la liste, faits empilés, tableau en fiches, « Fermer » | **Détail** `#espaces/<id>` : page (téléphone) ou panneau latéral (desktop) ; chiffres en grille, lignes d'appareils dépliables |
| « Agir sur cet espace depuis le serveur » | Vraies actions dans le détail (retirer, quota, supprimer) ; les commandes passent dans le repli discret « Équivalent en ligne de commande » |
| Invitations : phrase, liste, 2 boutons, bloc jeton en bas | Vue **Invitations** : jeton en haut (une fois), carte « Créer », carte « En attente » avec « Remplacer » discret, aide derrière « ? » |
| Quota par défaut : paragraphe, 3 faits, champ, 3 boutons | Vue **Réglages**, carte Quota : valeur en grand, champ + unité + Enregistrer, 2 actions discrètes, aide derrière « ? » |
| Journal d'administration | Vue **Journal** : heure à gauche, action à droite, refus en ocre avec le conseil et la commande ; aide derrière « ? » |
| À propos et diagnostic : faits empilés, limites dépliables, résumé affiché, Copier | Vue **Réglages** : « À propos » en lignes libellé / valeur, « Limites » replié, « Diagnostic » : Copier d'abord, résumé replié |
| Chargement, navigateur non pris en charge, enrôlement, connexion | Mêmes écrans, sans navigation, centrés sous le logo (`etats.html`) |

## Changements à faire pour porter la maquette (non faits)

Tout reste dans les trois fichiers servis par `admin_web.rs` (`include_str!`) : aucune route, aucune ressource
nouvelle, **aucun changement d'API** ni du serveur. La CSP actuelle (`style-src 'self'`, `script-src 'self'`,
`img-src 'self'`) est respectée à deux conditions notées plus bas.

### `index.html`
- Remplacer `header.top` + `nav.dash-nav` par : `aside.side` (logo, `nav.nav` à 5 liens, `section.session-panel`),
  `div.scrim`, `header.appbar` (Retour, logo 24 px, titre, « Mis à jour à », Actualiser, ⋮, filet de chargement).
- Découper `#view-dashboard` en cinq `section.view` (`view-apercu`, `view-espaces`, `view-invitations`,
  `view-journal`, `view-reglages`), une seule visible. Garder `view-loading`, `view-nojs`, `view-enroll`,
  `view-login`, habillés en `.auth`.
- Garder les identifiants que `app.js` utilise déjà (`refresh`, `updated-at`, `alerts`, `status-list`,
  `spaces-table`, `spaces-body`, `token-box`, `token`, `token-qr`, `quota-form`, `quota-mib`, `journal-list`,
  `diagnostic`, `confirm-dialog`, `toast`, `message`…) : le portage déplace des blocs, il n'en renomme pas.
- `#devices-section` devient `section.detail` dans `div.spaces-layout`, à côté de la liste ; `#devices-table`
  devient `ul.devices` ; `#space-facts` devient `dl.stats` + bloc d'usage.
- Thème : le `<select id="theme">` devient trois boutons radio `name="theme"` dans un `.segmented`.
- Ajouter le jeu d'icônes en ligne (`<svg>` de `<symbol>`), les « ? » (`button.help-btn` + `div.help-text`), le
  `<template id="tpl-device">`.
- CSP : aucun attribut `style=""` (la maquette en a quelques-uns, à remplacer par des classes), y compris sur le
  `<svg>` du jeu d'icônes.

### `app.css`
- Reprendre `maquette.css` jusqu'à la section « planche d'états » exclue (elle ne sert qu'à `etats.html`), à la
  place de la feuille actuelle. Les jetons de couleur sont les mêmes ; s'ajoutent les tailles (`--target`,
  `--bar-h`, `--nav-h`, `--side-w`, `--panel-w`) et les couleurs du retour d'action.
- Points de bascule : 640 px (deux colonnes, confirmation centrée), 1024 px (barre latérale, panneau de détail),
  1440 px (plus d'air). Le tableau des espaces bascule sur la largeur de **sa** zone (`@container`), à 480, 832
  et 1056 px.
- Supprimer `.dash-nav`, `.top`, `.tiles dt/dd`, `.facts`, `table.stack` et leurs règles mobiles.

### `app.js`
- **Routeur** par `location.hash` (`hashchange`) : affiche une vue, marque `aria-current="page"`, met à jour
  `<h1>` et `document.title`, donne le focus au titre. `selectSpace(id)` et `closeDevices()` deviennent des
  changements d'adresse ; `refreshAll()` ne touche pas à la vue courante. `show('dashboard')` garde la vue de
  l'adresse au lieu de revenir en haut.
- **Session** : `tickSession()` écrit dans la feuille (`#session-info`) et n'affiche la puce de la barre que si
  `minutes <= 5` ; ouverture et fermeture de la feuille (⋮, voile, Échap, retour du focus).
- **Thème** : `loadTheme()` / `saveTheme()` lisent les boutons radio au lieu du `<select>` (même clé
  `tomi-admin-theme`).
- **`renderStatus()`** : la tuile « Relais » disparaît (version et durée passent dans la carte d'état) ; tuiles
  en `div.tile` avec `.k`, `.v`, `.sub` ; tuiles Espaces et Invitations en liens.
- **`renderAlerts()`** : titre « Rien à signaler » / « N points d'attention » / « N problèmes », icône et classe
  selon la gravité la plus haute ; chaque alerte reçoit un lien (`#espaces/<id>`, `#journal`, `#reglages`).
- **`renderSpaces()`** : mêmes colonnes dans un autre ordre (identifiant, état, appareils, contenu, utilisation,
  activité, créé, action), classes `c-*` ; l'identifiant est un lien ; pastille « Quota à N % » quand
  `percent(used, max) >= 90` ; libellés courts « Clé à renouveler » et « Inactif » ; tri aussi par la liste
  déroulante (téléphone), reliée à `sorts.spaces`.
- **`spaceFacts()`** : quatre chiffres + jauge ; **retirer** `(n° … à …)` et ne garder que le compte
  d'événements avec `event_retention_days` (déjà dans `api/about`).
- **`renderDevices()`** : une ligne `<details>` par appareil depuis le gabarit ; état réduit à un mot ; icône
  selon `platform` ; tri par liste déroulante.
- **Invitations** : après création, afficher le bloc jeton en haut de la vue et y amener le focus ; lien replié.
  Le QR reste le `<canvas>` actuel (l'encodeur ne change pas) ; la maquette n'a qu'un motif factice.
- **Quota** : `renderQuota()` écrit la valeur en grand + l'origine en légende ; `previewQuota()` marque le refus
  (`.bad`, `aria-invalid`).
- **Journal** : pour `sign_in_refused` / `enrollment_refused`, ajouter la ligne de conseil avec la commande
  `tomi-relay admin web --rotate-path` (aujourd'hui dans l'alerte de la vue d'ensemble seulement).
- **Actualiser** : afficher le filet linéaire pendant `refreshAll()` ; « Réessayer » des erreurs = `refreshAll()`.
- **Copie** : `copyButton()` rend un bouton-icône (`aria-label` inchangé) au lieu de « Copier ».
- CSP : construire le DOM comme aujourd'hui (`textContent`, pas de `innerHTML`). La maquette utilise `innerHTML`
  par commodité : à ne pas recopier.

### Tests et documents
- `apps/relay/tests/admin_web.rs` vérifie le HTML servi : à relire après le portage (identifiants, textes).
- `docs/FONCTIONNALITES.md` : ligne du panneau d'administration (navigation, détail d'un espace, pastilles).

## Points à trancher par le propriétaire

1. **5 destinations ou 6** : fusionner Quota et Diagnostic dans « Réglages » (proposé), ou garder 6 entrées avec
   des libellés plus courts.
2. **Tri par défaut des espaces** : dernière activité, la plus récente d'abord (proposé) ; aujourd'hui, date de
   création croissante.
3. **Abandon des n° de séquence** dans le détail d'un espace (proposé : seulement le compte et la rétention).
4. **Pastille « Quota à N % »** : seuil à 90 % comme les alertes (proposé) ; et les libellés courts « Clé à
   renouveler » / « Inactif ».
5. **Bouton de la carte d'espace** : « Appareils » (libellé actuel, gardé) ou « Détail », puisqu'il ouvre aussi
   les chiffres et les commandes.
6. **QR visible d'office sur téléphone** (proposé : oui, 120 px, à côté de sa légende) ou replié comme le lien.
7. **Thème sur l'écran de connexion** : la maquette n'y met pas de réglage (le choix enregistré s'applique, sinon
   celui du système). À ajouter si tu changes souvent de thème avant de te connecter.
8. **Icônes** : tracés Material Symbols recopiés à la main dans la maquette ; au portage, reprendre les tracés
   officiels (Outlined, 24 px, graisse 400).

## Limites de la maquette

- Données fictives et figées (11 nov. 2026, 20:12) : espaces `794e87f1`, `2de8c1b2`, `380a4493`, relais v0.5.1,
  jeton `EXEMPLE-…`, domaine `relais.example`, QR factice non scannable.
- Les tris ne trient pas, « Actualiser » ne lit rien, « Se déconnecter » mène à `etats.html#connexion`.
- Actions : la clé d'accès est simulée (boutons « Maquette : la clé d'accès… »), les plafonds sont comptés dans la
  page et repartent de zéro au rechargement (une invitation est déjà comptée), les dates écrites après une action
  sont figées (« à l'instant », « jusqu'au 18/11 »). « Fermer toutes les sessions » et le cinquième refus mènent à
  `etats.html`.
- Pas de rendu sur appareil réel : vérifié dans un navigateur de bureau redimensionné (390 x 844, 1280 x 800 et
  1440 x 900), pas sur le Galaxy S23. La zone du geste système en bas d'écran est prévue
  (`env(safe-area-inset-bottom)`), non observée ; le clavier virtuel au-dessus du champ des 8 caractères non plus.