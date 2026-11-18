# Panneau d'administration du relais : décisions UI proposées (refonte mobile + desktop)

Maquette : `index.html` (prototype navigable), `etats.html` (planche des états : connexion, chargement, vide,
erreur, variantes), `maquette.css`, `maquette.js` (démonstration seulement). Statique, données fictives, aucune
requête réseau, aucune dépendance. Ouvrir `index.html` dans un navigateur ; le thème se change dans le pied de la
barre latérale (desktop) ou par ⋮ (téléphone).
Statut : **proposition**, rien n'est porté dans `apps/relay/src/admin_web/`.

Vérifié dans le navigateur de prévisualisation à 390 px et 1280 px, en clair et en sombre : aucune vue ne défile
horizontalement, aucun texte coupé, toutes les cibles tactiles font au moins 44 px (hors contrôle segmenté du
thème : 40 px dans un cadre de 44).

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
16. **Relais aveugle** : la maquette ne montre que ce que l'API renvoie déjà (comptes, tailles, dates,
    identifiants courts, nom d'appareil, empreinte). Aucune action destructrice n'est ajoutée : retirer un
    appareil ou supprimer un espace reste sur le serveur, les commandes se copient.

## Ancien → nouveau, section par section

| Ancien (une seule page) | Nouveau |
|---|---|
| En-tête : titre, « Session : encore 30 min », Thème (liste), Se déconnecter | Barre de 56 px ; session, thème et déconnexion dans la feuille ⋮ (téléphone) ou le pied de la barre latérale (desktop) |
| Onglets d'ancres (6, coupés) | Barre basse à 5 destinations / barre latérale ; une vue à la fois |
| Vue d'ensemble : « Mis à jour à », Actualiser, « Rien à signaler », 6 tuiles empilées, aide dépliable | Vue **Aperçu** : carte d'état + alertes cliquables, tuile stockage, 4 tuiles en grille ; « Actualiser » dans la barre ; version et durée de service dans la carte d'état |
| Espaces : cartes clé / valeur, bouton « Appareils » | Vue **Espaces** : cartes compactes (téléphone) ou tableau triable (desktop), tri par liste déroulante sur téléphone |
| Appareils de l'espace : section sous la liste, faits empilés, tableau en fiches, « Fermer » | **Détail** `#espaces/<id>` : page (téléphone) ou panneau latéral (desktop) ; chiffres en grille, lignes d'appareils dépliables |
| « Agir sur cet espace depuis le serveur » | Inchangé sur le fond : repli en bas du détail, une commande par ligne avec sa copie |
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
- Pas de rendu sur appareil réel : vérifié dans un navigateur de bureau redimensionné (390 x 844 et 1280 x 800),
  pas sur le Galaxy S23. La zone du geste système en bas d'écran est prévue (`env(safe-area-inset-bottom)`),
  non observée.