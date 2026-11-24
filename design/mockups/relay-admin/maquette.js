// Maquette du panneau d'administration du relais Tomi : comportement de démonstration seulement.
// Aucune requête réseau, aucune dépendance. Rien ici n'est du code de production : la navigation,
// le thème, les replis et les retours d'action sont simulés pour pouvoir juger l'interface.
// Sert à index.html (prototype navigable, téléphone et desktop) et à etats.html (planche d'états : thème, « ? »,
// copie, QR). Le parcours des actions (clé d'accès à chaque action qui détruit ou crée un droit) suit
// DECISION-ACTIONS.md ; la clé d'accès est simulée par des boutons propres à la maquette.
'use strict';

(function () {
  function $(id) {
    return document.getElementById(id);
  }

  function all(selector, root) {
    return Array.prototype.slice.call((root || document).querySelectorAll(selector));
  }

  function icon(name, extra) {
    return '<svg class="ico' + (extra ? ' ' + extra : '') + '" aria-hidden="true"><use href="#i-' + name + '"/></svg>';
  }

  // ----- thème : Automatique / Clair / Sombre, retenu dans ce navigateur -----
  var THEME_KEY = 'tomi-maquette-theme';

  function applyTheme(value) {
    if (value === 'light' || value === 'dark') {
      document.documentElement.setAttribute('data-theme', value);
    } else {
      document.documentElement.removeAttribute('data-theme');
      value = 'auto';
    }
    all('input[name="theme"]').forEach(function (input) {
      input.checked = input.value === value;
    });
  }

  var savedTheme = 'auto';
  try {
    savedTheme = window.localStorage.getItem(THEME_KEY) || 'auto';
  } catch (e) {
    savedTheme = 'auto';
  }
  applyTheme(savedTheme);
  all('input[name="theme"]').forEach(function (input) {
    input.addEventListener('change', function () {
      applyTheme(input.value);
      try {
        if (input.value === 'auto') {
          window.localStorage.removeItem(THEME_KEY);
        } else {
          window.localStorage.setItem(THEME_KEY, input.value);
        }
      } catch (e) {
        // Stockage indisponible : le choix vaut pour cette page.
      }
    });
  });

  // ----- retour d'action : un succès s'efface seul, une erreur reste jusqu'à « Fermer » -----
  var toastTimer = null;

  function say(text, kind) {
    var toast = $('toast');
    if (!toast) {
      return;
    }
    clearTimeout(toastTimer);
    if (!text) {
      toast.hidden = true;
      return;
    }
    var error = kind === 'error';
    toast.className = 'toast' + (error ? ' error' : '');
    toast.querySelector('use').setAttribute('href', error ? '#i-error' : '#i-check');
    $('message').setAttribute('role', error ? 'alert' : 'status');
    $('toast-close').hidden = !error;
    toast.hidden = false;
    $('message').textContent = text;
    if (!error) {
      toastTimer = setTimeout(function () {
        toast.hidden = true;
      }, 5000);
    }
  }

  // ----- le « ? » : l'explication reste repliée tant qu'on ne la demande pas -----
  all('.help-btn').forEach(function (button) {
    button.addEventListener('click', function () {
      var open = button.getAttribute('aria-expanded') !== 'true';
      button.setAttribute('aria-expanded', open ? 'true' : 'false');
      $(button.getAttribute('aria-controls')).hidden = !open;
    });
  });

  // ----- QR factice : un motif qui ressemble à un QR, non scannable (aucun jeton réel) -----
  all('svg.fake-qr, #token-qr').forEach(function (svg) {
    var n = 25;
    var seed = 20261111;
    var rects = '';

    function finder(x, y, col, row) {
      var dx = col - x;
      var dy = row - y;
      if (dx < 0 || dy < 0 || dx > 6 || dy > 6) {
        return null;
      }
      return Math.max(Math.abs(dx - 3), Math.abs(dy - 3)) !== 2;
    }

    for (var row = 0; row < n; row++) {
      for (var col = 0; col < n; col++) {
        var dark = finder(0, 0, col, row);
        if (dark === null) {
          dark = finder(n - 7, 0, col, row);
        }
        if (dark === null) {
          dark = finder(0, n - 7, col, row);
        }
        if (dark === null) {
          var quiet = (col < 8 && row < 8) || (col > n - 9 && row < 8) || (col < 8 && row > n - 9);
          seed = (seed * 1103515245 + 12345) & 0x7fffffff;
          dark = !quiet && (seed >> 16) % 2 === 0;
        }
        if (dark) {
          rects += '<rect x="' + (col + 4) + '" y="' + (row + 4) + '" width="1" height="1"/>';
        }
      }
    }
    svg.setAttribute('viewBox', '0 0 ' + (n + 8) + ' ' + (n + 8));
    svg.innerHTML = '<g fill="#000000">' + rects + '</g>';
  });

  // ----- copie : la maquette ne montre que le retour (« Identifiant copié ») -----
  document.addEventListener('click', function (event) {
    var button = event.target.closest ? event.target.closest('[data-copy]') : null;
    if (button) {
      event.preventDefault();
      say(button.getAttribute('data-copy') + ' copié');
    }
  });

  if ($('toast-close')) {
    $('toast-close').addEventListener('click', function () {
      say('');
    });
  }


  // Formulaires de démonstration (planche d'états) : rien n'est envoyé. Remplace onsubmit="", interdit par la CSP.
  all('form[data-inert]').forEach(function (form) {
    form.addEventListener('submit', function (event) {
      event.preventDefault();
    });
  });

  // Le reste ne concerne que le prototype navigable (index.html).
  if (!$('spaces-layout')) {
    return;
  }

  // ----- données fictives (11/11/2026, 20:12), mêmes chiffres que le tableau statique -----
  // Appareil : [nom, icône, plateforme · version · activité, état, classe, ajouté, notifications, empreinte]
  function dev(a) {
    return {
      name: a[0], icon: a[1], sub: a[2], state: a[3], cls: a[4], added: a[5], push: a[6], fp: a[7],
      id: a[7].replace(/ /g, '').slice(0, 8)
    };
  }

  var SPACES = {
    '794e87f1': {
      items: ['128 éléments', '14 fichiers chiffrés'],
      events: ['312 événements', 'retenus 45 j au plus'],
      created: ['il y a 2 mois', '12/09/2026 à 00:57'],
      act: ['il y a 12 min', '2026-11-11T20:00:41Z', '11/11/2026 20:00:41'],
      crt: ['il y a 2 mois', '2026-09-12T00:57:18Z', '12/09/2026 00:57:18'],
      used: 183, quota: 'default', rotation: false,
      devices: [
        ['LAPTOP-XPS', 'computer', 'Windows · 0.5.1 · il y a 12 min', 'Présent', 'good', 'il y a 2 mois', 'Sans notifications', '7c3e 91af 0b52 d4e8'],
        ['Galaxy S23', 'phone', 'Android · 0.5.1 · il y a 2 h', 'Actif', '', 'il y a 2 mois', 'Activées', 'a10b 44f2 9e7c 3d05'],
        ['iPhone 15', 'phone', 'iOS · 0.5.0 · hier', 'Actif', '', 'il y a 3 sem.', 'Activées', '5d92 c8e1 07ab 6f34'],
        ['PC-Bureau', 'computer', 'Windows · 0.4.2 · il y a 2 mois', 'Inactif', 'muted', 'il y a 2 mois', 'Sans notifications', 'e4f0 12cd 88a3 5b79']
      ].map(dev)
    },
    '2de8c1b2': {
      items: ['2 éléments', '0 fichier chiffré'],
      events: ['11 événements', 'retenus 45 j au plus'],
      created: ['hier', '10/11/2026 à 09:31'],
      act: ['il y a 2 h', '2026-11-11T18:10:02Z', '11/11/2026 18:10:02'],
      crt: ['hier', '2026-11-10T09:31:55Z', '10/11/2026 09:31:55'],
      used: 1.2, quota: 'default', rotation: false,
      devices: [
        ['Pixel 8', 'phone', 'Android · 0.5.1 · il y a 2 h', 'Actif', '', 'hier', 'Activées', '31c7 fa20 6d4e 9b18']
      ].map(dev)
    },
    '380a4493': {
      items: ['37 éléments', '61 fichiers chiffrés'],
      events: ['96 événements', 'retenus 45 j au plus'],
      created: ['il y a 4 mois', '03/07/2026 à 14:20'],
      act: ['avant-hier', '2026-11-09T21:44:10Z', '09/11/2026 21:44:10'],
      crt: ['il y a 4 mois', '2026-07-03T14:20:07Z', '03/07/2026 14:20:07'],
      used: 241, quota: 'default', rotation: false,
      devices: [
        ['ThinkPad-T14', 'computer', 'Windows · 0.5.0 · avant-hier', 'Actif', '', 'il y a 4 mois', 'Sans notifications', '9a6b 0e53 c1f7 24d8'],
        ['iPad', 'phone', 'iOS · 0.5.0 · il y a 6 j', 'Actif', '', 'il y a 3 mois', 'Activées', 'b2d4 7719 e06c 3a5f'],
        ['serveur-sauvegarde', 'terminal', 'CLI · 0.3.0 · il y a 3 mois', 'Inactif', 'muted', 'il y a 4 mois', 'Sans notifications', '0f8e 63b1 d7a2 4c90']
      ].map(dev)
    }
  };
  var ORDER = ['794e87f1', '2de8c1b2', '380a4493'];

  // État du relais (fictif). Plafonds comptés par relais (DECISION-ACTIONS.md, point 4).
  var RELAY = {
    defaultQuota: 256, // Mio, null = sans limite par espace
    invitations: [['c41d09aa', 'créée il y a 9 min', '2026-11-11T20:03:12Z']],
    used: { 'delete': 0, revoke: 0, invite: 1 },
    caps: { 'delete': 2, revoke: 10, invite: 5 },
    refusals: 0,
    nextToken: 1
  };

  function mib(value) {
    if (value === null) {
      return 'aucune limite';
    }
    if (value < 1024) {
      return String(value).replace('.', ',') + ' Mio';
    }
    return (value / 1024).toFixed(value >= 102400 ? 0 : 1).replace('.', ',') + ' Gio';
  }

  function limitOf(s) {
    if (s.quota === 'default') {
      return RELAY.defaultQuota;
    }
    return s.quota === 'none' ? null : s.quota;
  }

  function pctOf(s) {
    var limit = limitOf(s);
    return limit ? Math.round((s.used / limit) * 1000) / 10 : null;
  }

  function frPct(p) {
    return String(p).replace('.', ',') + ' %';
  }

  function counts(s) {
    var active = s.devices.filter(function (d) { return d.state !== 'Inactif'; }).length;
    var present = s.devices.filter(function (d) { return d.state === 'Présent'; }).length;
    return { active: active, present: present, total: s.devices.length };
  }

  function badgesOf(s) {
    var out = [];
    var p = pctOf(s);
    if (s.rotation) {
      out.push(['Clé à renouveler', 'warn']);
    }
    if (p !== null && p >= 100) {
      out.push(['Quota atteint', 'bad']);
    } else if (p !== null && p >= 90) {
      out.push(['Quota à ' + Math.round(p) + ' %', 'warn']);
    }
    if (counts(s).active === 0) {
      out.push(['Inactif', 'muted']);
    }
    return out.length ? out : [['Normal', 'good']];
  }

  function badgesHtml(s) {
    return badgesOf(s).map(function (b) {
      return '<span class="badge ' + b[1] + '">' + b[0] + '</span>';
    }).join('');
  }

  function usageHtml(s) {
    var limit = limitOf(s);
    var p = pctOf(s);
    if (limit === null) {
      return '<div class="usage-txt flush"><span><strong>' + mib(s.used) + '</strong> · aucune limite</span></div>';
    }
    return '<progress class="' + (p >= 100 ? 'full' : p >= 90 ? 'warn' : '') + '" max="100" value="' + Math.min(p, 100) +
      '" aria-label="Quota utilisé : ' + frPct(p) + '">' + frPct(p) + '</progress><div class="usage-txt"><span><strong>' +
      mib(s.used) + '</strong> sur ' + mib(limit) + '</span><span>' + frPct(p) + '</span></div>';
  }

  // ----- liste des espaces : même DOM que le tableau statique, refait depuis les données -----
  function renderSpaces() {
    var body = $('spaces-body');
    body.innerHTML = ORDER.map(function (id) {
      var s = SPACES[id];
      var c = counts(s);
      return '<tr data-space="' + id + '">' +
        '<th scope="row" class="c-id"><a class="space-link mono" href="#espaces/' + id + '">' + id + '</a>' +
        '<button type="button" class="icon-btn sm" data-copy="Identifiant" aria-label="Copier l’identifiant ' + id + '">' + icon('copy') + '</button></th>' +
        '<td class="c-state"><span class="badges">' + badgesHtml(s) + '</span></td>' +
        '<td class="c-dev" data-label="Appareils"><span class="v">' + c.active + ' actif' + (c.active > 1 ? 's' : '') + ' sur ' + c.total +
        '</span><span class="sub">' + c.present + ' présent' + (c.present > 1 ? 's' : '') + '</span></td>' +
        '<td class="c-items" data-label="Contenu"><span class="v">' + s.items[0] + '</span><span class="sub">' + s.items[1] + '</span></td>' +
        '<td class="c-usage">' + usageHtml(s) + '</td>' +
        '<td class="c-act"><time datetime="' + s.act[1] + '" title="' + s.act[2] + '">' + s.act[0] + '</time></td>' +
        '<td class="c-crt"><time datetime="' + s.crt[1] + '" title="' + s.crt[2] + '">' + s.crt[0] + '</time></td>' +
        '<td class="c-go"><a class="btn tonal sm" href="#espaces/' + id + '" aria-label="Appareils de l’espace ' + id + '"><span>Appareils</span>' +
        icon('chevron') + '</a></td></tr>';
    }).join('');
    var n = ORDER.length;
    $('spaces-count').textContent = n + ' espace' + (n > 1 ? 's' : '');
    $('tile-spaces').textContent = n;
    var total = 0;
    var active = 0;
    ORDER.forEach(function (id) {
      var c = counts(SPACES[id]);
      total += c.total;
      active += c.active;
    });
    $('tile-devices').textContent = total + ' appareils, ' + active + ' actifs';
    markSelectedRow();
  }

  function markSelectedRow() {
    all('#spaces-body tr').forEach(function (tr) {
      tr.classList.toggle('selected', tr.getAttribute('data-space') === lastSpace);
    });
  }

  $('spaces-body').addEventListener('click', function (event) {
    var tr = event.target.closest('tr');
    if (tr && !event.target.closest('a, button')) {
      window.location.hash = '#espaces/' + tr.getAttribute('data-space');
    }
  });

  // ----- détail d'un espace -----
  function pair(node, values) {
    node.textContent = values[0];
    var sub = document.createElement('span');
    sub.className = 'sub';
    sub.textContent = values[1];
    node.appendChild(sub);
  }

  function quotaOrigin(s) {
    if (s.quota === 'default') {
      return RELAY.defaultQuota === null ? 'Quota par défaut : aucune limite' : 'Quota par défaut (' + mib(RELAY.defaultQuota) + ')';
    }
    return s.quota === 'none' ? 'Sans limite, propre à cet espace' : 'Quota propre à cet espace';
  }

  function commandsFor(id, s) {
    var first = s.devices[0] ? s.devices[0].id : 'ID_APPAREIL';
    return [
      ['État détaillé de l’espace', 'tomi-relay admin status --space ' + id],
      ['Appareils, avec leur identifiant complet', 'tomi-relay admin devices --space ' + id],
      ['Quota propre à l’espace (OCTETS, none ou default)', 'tomi-relay admin quota --space ' + id + ' OCTETS'],
      ['Retirer un appareil', 'tomi-relay admin revoke --space ' + id + ' ' + first],
      ['Supprimer l’espace', 'tomi-relay admin reset --space ' + id + ' --confirm ' + id]
    ];
  }

  function fillDetail(id) {
    var s = SPACES[id];
    var c = counts(s);
    $('d-id').textContent = id;
    $('t-space').textContent = id;
    $('d-badges').innerHTML = badgesHtml(s);
    pair($('d-dev'), [c.active + ' actif' + (c.active > 1 ? 's' : '') + ' sur ' + c.total,
      c.present + ' présent' + (c.present > 1 ? 's' : '') + ' en ce moment']);
    pair($('d-items'), s.items);
    pair($('d-events'), s.events);
    pair($('d-created'), s.created);
    var limit = limitOf(s);
    var p = pctOf(s);
    $('d-progress').hidden = limit === null;
    if (limit !== null) {
      $('d-progress').value = Math.min(p, 100);
      $('d-progress').className = p >= 100 ? 'full' : p >= 90 ? 'warn' : '';
      $('d-progress').setAttribute('aria-label', 'Quota utilisé : ' + frPct(p));
      $('d-usage').innerHTML = '<strong>' + mib(s.used) + '</strong> sur ' + mib(limit);
      $('d-pct').textContent = frPct(p);
    } else {
      $('d-usage').innerHTML = '<strong>' + mib(s.used) + '</strong> utilisés';
      $('d-pct').textContent = 'aucune limite';
    }
    $('d-quota').textContent = quotaOrigin(s) + (p === null ? '' : p >= 100 ? ' · ses envois sont refusés' :
      p >= 90 ? ' · au-delà, les envois sont refusés' : '');
    closeQuotaForm();

    // Clé d'espace : le relais ne peut pas la renouveler ; il n'y a donc aucun bouton.
    var rot = $('d-rotation');
    rot.className = 'note-line' + (s.rotation ? ' warn' : '');
    rot.innerHTML = s.rotation ?
      icon('warning') + '<span><strong>Clé à renouveler</strong> après un retrait. Un appareil restant le fait tout seul à sa prochaine connexion ; le relais, qui n’a pas la clé, ne peut ni le faire ni le valider à sa place.</span>' :
      icon('check') + '<span>Clé de l’espace : rien en attente</span>';

    $('d-count').textContent = s.devices.length;
    var last = s.devices.length === 1;
    $('d-dev-hint').textContent = last ?
      'Dernier appareil de l’espace : il ne peut pas être retiré. Pour en finir avec cet espace, supprime-le.' :
      'Coche un ou plusieurs appareils pour les retirer : une seule confirmation avec ta clé d’accès.';

    var list = $('d-devices');
    list.innerHTML = '';
    s.devices.forEach(function (d) {
      var li = $('tpl-device').content.cloneNode(true);
      var root = li.querySelector('li');
      root.setAttribute('data-device', d.id);
      li.querySelector('summary use').setAttribute('href', '#i-' + d.icon);
      li.querySelector('.name').textContent = d.name;
      li.querySelector('.sub').textContent = d.sub;
      li.querySelector('.badge').textContent = d.state;
      li.querySelector('.badge').className = 'badge ' + d.cls;
      li.querySelector('.f-added').textContent = d.added;
      li.querySelector('.f-push').textContent = d.push;
      li.querySelector('.f-id').textContent = d.id;
      li.querySelector('.f-fp').textContent = d.fp;
      li.querySelector('[data-copy]').setAttribute('aria-label', 'Copier l’empreinte de ' + d.name);
      var box = li.querySelector('.dev-sel');
      box.value = d.id;
      box.disabled = last;
      li.querySelector('.dev-check .sr').textContent = 'Sélectionner ' + d.name;
      li.querySelector('.dev-revoke').hidden = last;
      li.querySelector('.dev-last').hidden = !last;
      li.querySelector('.dev-quick').hidden = last;
      li.querySelector('.dev-quick').setAttribute('aria-label', 'Retirer ' + d.name + ' de l’espace');
      li.querySelector('.dev-quick').setAttribute('title', 'Retirer ' + d.name);
      list.appendChild(li);
    });
    syncSelection();

    $('d-commands').innerHTML = commandsFor(id, s).map(function (cmd) {
      return '<li><span class="what">' + cmd[0] + '</span><div class="cmd"><code>' + cmd[1] +
        '</code><button type="button" class="icon-btn" data-copy="Commande" aria-label="Copier la commande : ' +
        cmd[0] + '">' + icon('copy') + '</button></div></li>';
    }).join('');
  }

  // ----- sélection d'appareils : un seul retrait (une seule clé d'accès) pour toute la liste -----
  function selectedIds() {
    return all('#d-devices .dev-sel:checked').map(function (box) { return box.value; });
  }

  function syncSelection() {
    var ids = selectedIds();
    var s = SPACES[lastSpace];
    all('#d-devices .dev').forEach(function (li) {
      li.classList.toggle('selected', li.querySelector('.dev-sel').checked);
    });
    var bar = $('d-selbar');
    bar.hidden = ids.length === 0;
    if (!ids.length) {
      return;
    }
    var everyone = s && ids.length === s.devices.length;
    var count = $('d-selcount');
    count.className = everyone ? 'bad' : '';
    count.textContent = everyone ?
      'Il doit rester au moins un appareil pour renouveler la clé. Pour retirer tout le monde, supprime l’espace.' :
      ids.length + ' appareil' + (ids.length > 1 ? 's' : '') + ' coché' + (ids.length > 1 ? 's' : '');
    $('d-selrevoke').disabled = everyone;
  }

  $('d-devices').addEventListener('change', function (event) {
    if (event.target.classList.contains('dev-sel')) {
      syncSelection();
    }
  });
  $('d-devices').addEventListener('click', function (event) {
    var button = event.target.closest('.dev-revoke, .dev-quick');
    if (button) {
      askRevoke([button.closest('.dev').getAttribute('data-device')], button);
    }
  });
  $('d-selclear').addEventListener('click', function () {
    all('#d-devices .dev-sel').forEach(function (box) { box.checked = false; });
    syncSelection();
  });
  $('d-selrevoke').addEventListener('click', function () {
    askRevoke(selectedIds(), $('d-selrevoke'));
  });

  // ----- navigation entre sections (#apercu, #espaces, #espaces/794e87f1, ...) -----
  var VIEWS = {
    apercu: 'Vue d’ensemble',
    espaces: 'Espaces',
    invitations: 'Invitations',
    journal: 'Journal',
    reglages: 'Réglages'
  };
  var desktop = window.matchMedia('(min-width: 64rem)');
  var lastView = null;
  var lastSpace = null;

  function route() {
    var parts = window.location.hash.replace(/^#/, '').split('/');
    var name = VIEWS[parts[0]] ? parts[0] : 'apercu';
    var space = name === 'espaces' && SPACES[parts[1]] ? parts[1] : null;

    Object.keys(VIEWS).forEach(function (v) {
      $('view-' + v).hidden = v !== name;
    });
    all('.nav a').forEach(function (a) {
      if (a.getAttribute('data-view') === name) {
        a.setAttribute('aria-current', 'page');
      } else {
        a.removeAttribute('aria-current');
      }
    });
    $('t-main').textContent = VIEWS[name];
    document.title = (space ? 'Espace ' + space : VIEWS[name]) + ' · Relais Tomi (maquette)';

    var changedSpace = space !== lastSpace;
    lastSpace = space;
    if (space && changedSpace) {
      fillDetail(space);
    }
    $('space-detail').hidden = !space;
    markSelectedRow();
    document.body.classList.toggle('has-detail', !!space);
    $('spaces-layout').classList.toggle('has-detail', !!space);

    closeSession();
    // Sur téléphone, le détail est une page : on repart du haut. Sur desktop, la liste ne bouge pas.
    if (name !== lastView || (changedSpace && !desktop.matches)) {
      window.scrollTo(0, 0);
    }
    if (space && changedSpace && desktop.matches) {
      $('h-detail').focus({ preventScroll: true });
    }
    lastView = name;
  }

  window.addEventListener('hashchange', route);
  $('back').addEventListener('click', function () {
    window.location.hash = '#espaces';
  });

  // ----- feuille de session (téléphone) : ouverte par le bouton ⋮ -----
  var panel = $('session-panel');
  var scrim = $('scrim');
  var menu = $('menu');

  function openSession() {
    panel.classList.add('open');
    scrim.hidden = false;
    menu.setAttribute('aria-expanded', 'true');
    $('session-close').focus();
  }

  function closeSession() {
    if (!panel.classList.contains('open')) {
      return;
    }
    panel.classList.remove('open');
    scrim.hidden = true;
    menu.setAttribute('aria-expanded', 'false');
  }

  menu.addEventListener('click', openSession);
  scrim.addEventListener('click', closeSession);
  $('session-close').addEventListener('click', function () {
    closeSession();
    menu.focus();
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && panel.classList.contains('open')) {
      closeSession();
      menu.focus();
    }
  });

  // « Ce que le relais voit » : toujours ouvert sur desktop, replié sur téléphone.
  var blind = $('blind');

  function syncBlind() {
    blind.open = desktop.matches;
  }

  syncBlind();
  if (desktop.addEventListener) {
    desktop.addEventListener('change', syncBlind);
  }
  $('open-blind').addEventListener('click', function () {
    setTimeout(function () {
      blind.open = true;
      blind.scrollIntoView({ block: 'center' });
    }, 0);
  });

  // ----- actualiser : filet linéaire sous la barre, puis l'heure de mise à jour -----
  function refresh() {
    var button = $('refresh');
    if (button.disabled) {
      return;
    }
    $('loading').hidden = false;
    button.disabled = true;
    button.textContent = 'Actualisation…';
    setTimeout(function () {
      $('loading').hidden = true;
      button.disabled = false;
      button.textContent = 'Actualiser';
      var now = new Date();
      var hm = ('0' + now.getHours()).slice(-2) + ':' + ('0' + now.getMinutes()).slice(-2);
      all('.updated-time').forEach(function (node) {
        node.textContent = hm;
      });
      if (!desktop.matches) {
        say('Mis à jour à ' + hm);
      }
    }, 900);
  }

  $('refresh').addEventListener('click', refresh);

  // ----- journal : les actions sensibles s'y ajoutent (identifiant court, numéro de clé) -----
  function log(what, small, kept) {
    var li = document.createElement('li');
    if (kept) {
      li.className = 'kept';
    }
    li.innerHTML = '<time datetime="2026-11-11T20:13:00Z" title="11/11/2026 20:13:00">à l’instant</time><span class="what">' +
      what + (small ? '<small>' + small + '</small>' : '') + '</span>';
    var list = $('journal-list');
    list.insertBefore(li, list.firstChild);
    $('journal-count').textContent = list.children.length;
  }

  // ----- confirmation simple (session seule) : Annuler a le focus, le bouton dit ce qu'il fait -----
  var dialog = $('confirm-dialog');
  var onConfirm = null;
  var returnFocus = null;

  function confirmAction(title, text, okLabel, then, options) {
    options = options || {};
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = okLabel;
    $('confirm-ok').className = 'btn ' + (options.tone || 'danger');
    $('confirm-cancel').textContent = options.cancel || 'Annuler';
    var list = $('confirm-list');
    list.hidden = !options.list;
    list.innerHTML = (options.list || []).map(function (row) {
      return '<li><span class="mono">' + row[0] + '</span><span>' + row[1] + '</span></li>';
    }).join('');
    onConfirm = then;
    returnFocus = document.activeElement;
    dialog.showModal();
    $('confirm-cancel').focus();
  }

  function restoreFocus() {
    if (returnFocus && document.contains(returnFocus)) {
      returnFocus.focus();
    }
  }

  $('confirm-cancel').addEventListener('click', function () {
    dialog.close();
  });
  dialog.addEventListener('close', restoreFocus);
  $('confirm-ok').addEventListener('click', function () {
    dialog.close();
    if (onConfirm) {
      onConfirm();
    }
  });

  // =====================================================================================
  // Confirmation avec clé d'accès (step-up), DECISION-ACTIONS.md : un défi par action, 2 min,
  // récapitulatif fourni par le serveur, 5 refus ferment la session. La maquette simule la clé.
  // =====================================================================================
  var su = $('stepup');
  var current = null;
  var countdown = null;
  var MESSAGES = {
    cancelled: ['warn', 'warning', 'Annulé sur ton appareil', 'Rien n’a été fait. Tu peux réessayer quand tu veux.'],
    refused: ['', 'error', 'Clé d’accès refusée par le relais',
      'Cette clé n’est pas enregistrée sur ce relais, ou la vérification (empreinte, visage, code) n’a pas eu lieu. Rien n’a été fait.'],
    expired: ['warn', 'schedule', 'Délai de 2 min dépassé',
      'Ce défi n’est plus valable et rien n’a été fait. Recommence pour en obtenir un nouveau.'],
    error: ['', 'error', 'Le relais n’a pas répondu',
      'L’action a pu être faite, ou non. Actualise pour voir l’état réel avant de recommencer.'],
    closed: ['', 'lock', 'Session fermée par prudence',
      '5 refus de clé d’accès dans cette session. Ta clé d’accès reste valable : reconnecte-toi, puis regarde le journal si ce n’était pas toi.']
  };

  function li(cls, text) {
    return '<li' + (cls ? ' class="' + cls + '"' : '') + '>' + text + '</li>';
  }

  function setSu(state, extra) {
    su.setAttribute('data-state', state);
    clearInterval(countdown);
    var ok = $('su-ok');
    var cancel = $('su-cancel');
    var msg = $('su-msg');
    cancel.textContent = state === 'review' || state === 'pending' ? 'Annuler' : 'Fermer';
    ok.className = 'btn ' + (current.tone || 'danger');
    ok.querySelector('svg').removeAttribute('hidden');
    $('su-input').disabled = state === 'pending';
    $('su-cli').open = state === 'limit';

    if (state === 'review') {
      $('su-ok-label').textContent = current.okLabel;
      syncTyped();
    } else if (state === 'pending') {
      var left = 120;
      $('su-countdown').textContent = '2:00';
      countdown = setInterval(function () {
        left -= 1;
        $('su-countdown').textContent = Math.floor(left / 60) + ':' + ('0' + (left % 60)).slice(-2);
        if (left <= 0) {
          setSu('expired');
        }
      }, 1000);
      cancel.focus();
    } else if (state === 'limit') {
      msg.className = 'su-msg warn';
      msg.innerHTML = '<strong>' + icon('schedule') + 'Limite atteinte : ' + extra.cap + '</strong><p>' + extra.next +
        ' Rien n’a été fait. Si c’est urgent, fais-le sur le serveur :</p>';
      cancel.focus();
    } else {
      var m = MESSAGES[state];
      msg.className = 'su-msg' + (m[0] ? ' ' + m[0] : '');
      var html = '<strong>' + icon(m[1]) + m[2] + '</strong><p>' + m[3] + '</p>';
      if (state === 'refused') {
        var bars = '';
        for (var i = 1; i <= 5; i++) {
          bars += '<span class="' + (i <= RELAY.refusals ? 'used' : '') + '"></span>';
        }
        html += '<div class="tries" aria-hidden="true">' + bars + '</div><p>Refus ' + RELAY.refusals +
          ' sur 5 : au cinquième, cette session se ferme. Ta clé d’accès, elle, reste valable.</p>';
      }
      msg.innerHTML = html;
      $('su-ok-label').textContent = state === 'expired' ? 'Recommencer' : state === 'error' ? 'Actualiser' :
        state === 'closed' ? 'Revenir à la connexion' : 'Réessayer';
      ok.disabled = false;
      if (state === 'error' || state === 'closed') {
        ok.className = 'btn';
        ok.querySelector('svg').setAttribute('hidden', '');
      }
      (state === 'closed' ? ok : cancel).focus();
    }
  }

  // Saisie des 8 premiers caractères (suppression d'un espace) : le bouton attend la bonne valeur.
  function syncTyped() {
    var ok = $('su-ok');
    if (!current || !current.expect) {
      ok.disabled = false;
      return;
    }
    var value = $('su-input').value.trim().toLowerCase();
    var good = value === current.expect;
    ok.disabled = !good;
    var out = $('su-match');
    out.className = 'caption' + (good ? ' ok' : '');
    out.textContent = good ? 'Identifiant correct' : value.length === 8 ? 'Ce n’est pas cet espace' : '';
    $('su-input').setAttribute('aria-invalid', value.length === 8 && !good ? 'true' : 'false');
  }

  $('su-input').addEventListener('input', syncTyped);

  function openStepup(cfg) {
    current = cfg;
    $('su-title').textContent = cfg.title;
    $('su-ico').className = 'su-ico' + (cfg.tone === '' ? ' create' : '');
    $('su-ico').innerHTML = icon(cfg.icon, 'lg');
    var recap = $('su-recap');
    recap.className = 'recap' + (cfg.facts ? ' facts' : '');
    recap.hidden = !cfg.recap.length;
    recap.innerHTML = cfg.recap.join('');
    $('su-conseq').innerHTML = cfg.conseq.map(function (c) { return li(c[1], c[0]); }).join('');
    $('su-type').hidden = !cfg.expect;
    $('su-input').value = '';
    $('su-expect').textContent = cfg.expect || '';
    $('su-pk-text').textContent = cfg.pk;
    $('su-cmd').textContent = cfg.cmd;
    $('su-cmd-note').textContent = cfg.cmdNote || '';
    $('su-cmd-note').hidden = !cfg.cmdNote;
    returnFocus = cfg.from || document.activeElement;
    su.showModal();
    if (cfg.direct) {
      start();
    } else {
      setSu('review');
      $('su-cancel').focus();
    }
  }

  // « api/stepup/options » : le serveur revérifie la cible et le plafond avant de fabriquer le défi.
  function start() {
    var kind = current.cap;
    if (kind && RELAY.used[kind] >= RELAY.caps[kind]) {
      setSu('limit', current.limit);
      return;
    }
    setSu('pending');
  }

  function closeSu() {
    clearInterval(countdown);
    if (su.open) {
      su.close();
    }
  }

  su.addEventListener('close', function () {
    clearInterval(countdown);
    restoreFocus();
  });
  $('su-cancel').addEventListener('click', closeSu);
  $('su-ok').addEventListener('click', function () {
    var state = su.getAttribute('data-state');
    if (state === 'error') {
      closeSu();
      refresh();
    } else if (state === 'closed') {
      window.location.href = 'etats.html#session-fermee';
    } else {
      start();
    }
  });
  all('[data-sim]').forEach(function (button) {
    button.addEventListener('click', function () {
      var result = button.getAttribute('data-sim');
      if (result === 'ok') {
        RELAY.used[current.cap] += 1;
        var done = current.done;
        closeSu();
        done();
      } else if (result === 'refused') {
        RELAY.refusals += 1;
        setSu(RELAY.refusals >= 5 ? 'closed' : 'refused');
      } else {
        setSu(result);
      }
    });
  });

  // ----- retirer un ou plusieurs appareils -----
  function askRevoke(ids, from) {
    var id = lastSpace;
    var s = SPACES[id];
    var devices = s.devices.filter(function (d) { return ids.indexOf(d.id) !== -1; });
    var one = devices.length === 1;
    openStepup({
      cap: 'revoke',
      from: from,
      icon: 'person-remove',
      title: one ? 'Retirer ' + devices[0].name + ' de l’espace ' + id + ' ?' :
        'Retirer ' + devices.length + ' appareils de l’espace ' + id + ' ?',
      recap: devices.map(function (d) {
        return '<li>' + icon(d.icon) + '<span class="r-main"><strong>' + d.name + '</strong><span class="caption">' + d.sub +
          '</span></span><span class="mono r-id">' + d.id + '</span></li>';
      }),
      conseq: [
        [one ? 'Il perd l’accès tout de suite. Pour revenir, il devra être recouplé depuis un autre appareil.' :
          'Ils perdent l’accès tout de suite. Pour revenir, ils devront être recouplés depuis un autre appareil.', 'strong'],
        [one ? 'Ses données restent lisibles sur lui : un retrait n’efface rien.' :
          'Leurs données restent lisibles sur eux : un retrait n’efface rien.', ''],
        ['Un appareil restant renouvellera la clé de l’espace à sa prochaine connexion.', 'good']
      ],
      pk: 'Ta clé d’accès te sera demandée · 10 retraits par heure au plus sur ce relais',
      okLabel: 'Retirer avec ma clé d’accès',
      cmd: devices.map(function (d) { return 'tomi-relay admin revoke --space ' + id + ' ' + d.id; }).join('\n'),
      cmdNote: 'Sur le serveur, la commande agit tout de suite, sans autre confirmation.',
      limit: { cap: '10 retraits d’appareil par heure', next: 'Le prochain sera possible à 21:04.' },
      done: function () {
        s.devices = s.devices.filter(function (d) { return ids.indexOf(d.id) === -1; });
        s.rotation = true;
        devices.forEach(function (d) {
          log('Appareil <span class="mono">' + d.id + '</span> retiré de l’espace <span class="mono">' + id + '</span>', 'clé 1 · gardé 90 j', true);
        });
        renderSpaces();
        fillDetail(id);
        say((one ? devices[0].name + ' retiré' : devices.length + ' appareils retirés') + ' · un appareil restant renouvellera la clé');
      }
    });
  }

  // ----- supprimer un espace : saisie des 8 caractères + clé d'accès, puis corbeille de 7 jours -----
  $('d-delete').addEventListener('click', function () {
    var id = lastSpace;
    var s = SPACES[id];
    var c = counts(s);
    openStepup({
      cap: 'delete',
      from: $('d-delete'),
      icon: 'delete',
      title: 'Supprimer l’espace ' + id + ' ?',
      facts: true,
      expect: id,
      recap: [
        li('', '<span>Appareils</span><span>' + c.total + ', dont ' + c.active + ' actif' + (c.active > 1 ? 's' : '') + '</span>'),
        li('', '<span>Contenu</span><span>' + s.items[0] + ' · ' + mib(s.used) + '</span>'),
        li('', '<span>Dernière activité</span><span>' + s.act[0] + '</span>'),
        li('', '<span>Créé</span><span>' + s.created[1].split(' ')[0] + '</span>')
      ],
      conseq: [
        [(c.total > 1 ? 'Ses ' + c.total + ' appareils ne le trouvent plus' : 'Son appareil ne le trouve plus') +
          ', tout de suite : plus aucune synchronisation.', 'strong'],
        ['Rien n’est effacé sur les appareils, et aucune invitation n’est créée à la place.', ''],
        ['Le relais garde le dossier chiffré 7 jours, jusqu’au 18/11. Seul le serveur peut le restaurer.', 'good']
      ],
      pk: 'Ta clé d’accès te sera demandée · 2 suppressions par 24 h au plus sur ce relais',
      okLabel: 'Supprimer avec ma clé d’accès',
      cmd: 'tomi-relay admin reset --space ' + id + ' --confirm ' + id,
      cmdNote: 'Attention : sur le serveur, l’effacement est immédiat, sans corbeille.',
      limit: { cap: '2 suppressions d’espace par 24 h', next: 'La prochaine sera possible demain à 20:13.' },
      done: function () {
        delete SPACES[id];
        ORDER.splice(ORDER.indexOf(id), 1);
        log('Espace <span class="mono">' + id + '</span> supprimé, mis à la corbeille', 'clé 1 · gardé 90 j', true);
        var trash = $('trash-list');
        var row = document.createElement('li');
        row.innerHTML = '<span><span class="mono">' + id + '</span><span class="caption block">supprimé à l’instant · ' + mib(s.used) +
          '</span></span><span class="muted nowrap">effacé dans 7 j</span>';
        trash.insertBefore(row, trash.firstChild);
        $('trash-count').textContent = trash.children.length;
        $('banner-id').textContent = id;
        $('banner-cmd').textContent = 'tomi-relay admin trash restore ' + id;
        $('spaces-banner').hidden = false;
        renderSpaces();
        window.location.hash = '#espaces';
        $('spaces-banner').focus();
      }
    });
  });
  $('spaces-banner').setAttribute('tabindex', '-1');
  $('banner-close').addEventListener('click', function () {
    $('spaces-banner').hidden = true;
  });

  // ----- invitations : créer et remplacer demandent la clé d'accès, invalider la session seule -----
  function showToken() {
    var tokens = ['EXEMPLE-7Q2M-K9XD-4TZA', 'EXEMPLE-3HVB-W8PN-6CRE', 'EXEMPLE-Z5LD-2QKT-9MXA'];
    $('token').textContent = tokens[RELAY.nextToken % tokens.length];
    RELAY.nextToken += 1;
    $('token-box').hidden = false;
    window.scrollTo(0, 0);
    $('token-title').focus({ preventScroll: true });
  }

  function renderInvitations() {
    var n = RELAY.invitations.length;
    $('invitations-list').innerHTML = RELAY.invitations.map(function (inv) {
      return '<li><span>Invitation <span class="mono">' + inv[0] + '</span></span><time class="muted nowrap" datetime="' + inv[2] +
        '">' + inv[1] + '</time></li>';
    }).join('');
    $('invitations-list').hidden = !n;
    $('inv-empty').hidden = !!n;
    $('inv-actions').hidden = !n;
    $('inv-count').textContent = n;
    $('tile-inv').textContent = n;
  }

  var INV_IDS = ['e7a2f30b', '5b94c1d8', '0d6e8a47', '9f13b2ce', 'a8c05e61'];

  function newInvitation() {
    return [INV_IDS[RELAY.nextToken % INV_IDS.length], 'créée à l’instant', '2026-11-11T20:13:00Z'];
  }

  var INVITE_LIMIT = { cap: '5 invitations par heure', next: 'La prochaine sera possible à 20:41.' };

  $('invite-new').addEventListener('click', function () {
    openStepup({
      cap: 'invite',
      direct: true, // « Créer » → clé d'accès tout de suite → jeton (aucune étape de plus)
      tone: '',
      icon: 'add',
      title: 'Créer une invitation',
      recap: [],
      conseq: [['Son jeton permettra à qui l’a de créer un espace sur ton relais. Il s’affichera une seule fois.', '']],
      pk: 'Ta clé d’accès te sera demandée · 5 invitations par heure au plus',
      okLabel: 'Créer avec ma clé d’accès',
      cmd: 'tomi-relay admin invite --new',
      limit: INVITE_LIMIT,
      done: function () {
        var inv = newInvitation();
        RELAY.invitations.unshift(inv);
        renderInvitations();
        log('Invitation créée', 'clé 1 · gardé 90 j', true);
        showToken();
        say('Invitation créée');
      }
    });
  });

  $('invite-rotate').addEventListener('click', function () {
    var n = RELAY.invitations.length;
    openStepup({
      cap: 'invite',
      icon: 'invite',
      title: 'Remplacer les invitations en attente ?',
      recap: RELAY.invitations.map(function (inv) {
        return '<li>' + icon('invite') + '<span class="r-main"><strong>Invitation <span class="mono">' + inv[0] +
          '</span></strong><span class="caption">' + inv[1] + '</span></span><span></span></li>';
      }),
      conseq: [
        [(n > 1 ? 'Ces ' + n + ' invitations cessent' : 'Cette invitation cesse') +
          ' de fonctionner : un jeton déjà donné ne permettra plus de créer d’espace.', 'strong'],
        ['Une nouvelle invitation est créée ; son jeton s’affichera une seule fois.', ''],
        ['Les espaces existants ne changent pas.', 'good']
      ],
      pk: 'Ta clé d’accès te sera demandée : un nouveau jeton est créé',
      okLabel: 'Remplacer avec ma clé d’accès',
      cmd: 'tomi-relay admin invite --rotate\ntomi-relay admin invite --new',
      limit: INVITE_LIMIT,
      done: function () {
        RELAY.invitations = [newInvitation()];
        renderInvitations();
        log('Invitations remplacées', n + ' invalidée' + (n > 1 ? 's' : '') + ' · clé 1 · gardé 90 j', true);
        showToken();
        say('Invitations remplacées : ' + n + ' invalidée' + (n > 1 ? 's' : '') + ', 1 créée');
      }
    });
  });

  $('invite-cancel').addEventListener('click', function () {
    var n = RELAY.invitations.length;
    confirmAction(
      n > 1 ? 'Invalider les ' + n + ' invitations en attente ?' : 'Invalider l’invitation en attente ?',
      'Un jeton déjà donné ne permettra plus de créer d’espace. Aucune invitation n’est créée à la place. Les espaces existants ne changent pas.',
      'Invalider',
      function () {
        RELAY.invitations = [];
        renderInvitations();
        $('token-box').hidden = true;
        log('Invitations invalidées', n + ' · gardé 90 j', true);
        say(n > 1 ? n + ' invitations invalidées' : 'Invitation invalidée');
      },
      { list: RELAY.invitations.map(function (inv) { return [inv[0], inv[1]]; }) }
    );
  });

  $('token-hide').addEventListener('click', function () {
    $('token-box').hidden = true;
    $('invite-new').focus();
  });

  // ----- quotas : session seule (réversible), confirmation si un espace serait bloqué -----
  function parseMib(text) {
    text = text.trim();
    return /^[0-9]{1,9}$/.test(text) && Number(text) >= 1 ? Number(text) : null;
  }

  function blockedBy(limit, onlyDefault) {
    return ORDER.filter(function (id) {
      var s = SPACES[id];
      return limit !== null && (!onlyDefault || s.quota === 'default') && s.used > limit;
    });
  }

  function afterQuota() {
    renderSpaces();
    if (lastSpace) {
      fillDetail(lastSpace);
    }
  }

  // Quota d'un espace (nouveau sur le web).
  function closeQuotaForm() {
    $('sq-form').hidden = true;
    $('sq-edit').setAttribute('aria-expanded', 'false');
    $('sq-edit').hidden = false;
  }

  function previewSpaceQuota() {
    var s = SPACES[lastSpace];
    var out = $('sq-preview');
    var raw = $('sq-mib').value.trim();
    var value = parseMib(raw);
    out.className = 'caption preview';
    if (!raw) {
      out.textContent = '';
    } else if (value === null) {
      out.classList.add('bad');
      out.textContent = 'Un nombre entier de Mio, au moins 1';
    } else if (value < s.used) {
      out.classList.add('warn');
      out.textContent = '= ' + mib(value) + ' · sous l’usage actuel (' + mib(s.used) + ') : ses envois seront refusés';
    } else {
      out.textContent = '= ' + mib(value) + ' pour cet espace';
    }
  }

  $('sq-edit').addEventListener('click', function () {
    var s = SPACES[lastSpace];
    $('sq-form').hidden = false;
    $('sq-edit').hidden = true;
    $('sq-edit').setAttribute('aria-expanded', 'true');
    $('sq-mib').value = typeof s.quota === 'number' ? s.quota : (RELAY.defaultQuota || '');
    $('sq-default').hidden = s.quota === 'default';
    previewSpaceQuota();
    $('sq-mib').focus();
  });
  $('sq-mib').addEventListener('input', previewSpaceQuota);

  function setSpaceQuota(id, quota, message) {
    SPACES[id].quota = quota;
    afterQuota();
    log('Quota de l’espace <span class="mono">' + id + '</span> : ' + (quota === 'default' ? 'par défaut' : mib(quota === 'none' ? null : quota)));
    say(message);
  }

  function confirmBlock(id, limit, then) {
    var s = SPACES[id];
    confirmAction(
      'Bloquer les envois de ' + id + ' ?',
      'Il utilise ' + mib(s.used) + ', plus que ' + mib(limit) + ' : ses appareils ne pourront plus rien envoyer tant qu’ils n’auront pas libéré de place. Rien n’est effacé, et tu peux revenir en arrière à tout moment.',
      'Appliquer ' + mib(limit),
      then
    );
  }

  $('sq-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var id = lastSpace;
    var value = parseMib($('sq-mib').value);
    if (value === null) {
      say('Saisis un nombre entier de Mio (au moins 1)', 'error');
      $('sq-mib').focus();
      return;
    }
    var apply = function () { setSpaceQuota(id, value, 'Quota de ' + id + ' : ' + mib(value)); };
    if (value < SPACES[id].used) {
      confirmBlock(id, value, apply);
    } else {
      apply();
    }
  });
  $('sq-default').addEventListener('click', function () {
    var id = lastSpace;
    var apply = function () { setSpaceQuota(id, 'default', 'Quota de ' + id + ' : retour au quota par défaut'); };
    if (RELAY.defaultQuota !== null && SPACES[id].used > RELAY.defaultQuota) {
      confirmBlock(id, RELAY.defaultQuota, apply);
    } else {
      apply();
    }
  });
  $('sq-none').addEventListener('click', function () {
    var id = lastSpace;
    confirmAction(
      'Retirer la limite de ' + id + ' ?',
      'Seul le total du relais (1,0 Gio) le limitera : il pourrait le remplir pour les autres espaces.',
      'Retirer la limite',
      function () { setSpaceQuota(id, 'none', 'Quota de ' + id + ' : sans limite'); }
    );
  });

  // Quota par défaut (inchangé), plus la confirmation conditionnelle avec la liste des espaces bloqués.
  var quota = $('quota-mib');

  function previewQuota() {
    var out = $('quota-preview');
    var raw = quota.value.trim();
    var value = parseMib(raw);
    out.className = 'caption preview';
    if (!raw) {
      out.textContent = '';
    } else if (value === null) {
      out.classList.add('bad');
      out.textContent = 'Un nombre entier de Mio, au moins 1';
    } else {
      var blocked = blockedBy(value, true);
      if (blocked.length) {
        out.classList.add('warn');
      }
      out.textContent = '= ' + mib(value) + ' par espace' + (blocked.length ?
        ' · ' + blocked.length + ' espace' + (blocked.length > 1 ? 's' : '') + ' au-dessus' : '');
    }
  }

  function setDefaultQuota(value) {
    RELAY.defaultQuota = value;
    $('quota-now').textContent = value === null ? 'Aucune limite par espace' : mib(value) + ' par espace';
    afterQuota();
  }

  quota.addEventListener('input', previewQuota);
  $('quota-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var value = parseMib(quota.value);
    if (value === null) {
      say('Saisis un nombre entier de Mio (au moins 1)', 'error');
      quota.focus();
      return;
    }
    var apply = function () {
      setDefaultQuota(value);
      log('Quota par défaut réglé : ' + mib(value) + ' par espace');
      say('Quota par défaut enregistré : ' + mib(value) + ' par espace');
    };
    var blocked = blockedBy(value, true);
    if (!blocked.length) {
      apply();
      return;
    }
    confirmAction(
      blocked.length > 1 ? 'Bloquer les envois de ' + blocked.length + ' espaces ?' : 'Bloquer les envois d’un espace ?',
      'Avec ' + mib(value) + ' par espace, ' + (blocked.length > 1 ? 'ces espaces dépassent' : 'cet espace dépasse') +
        ' déjà le plafond : leurs appareils ne pourront plus rien envoyer tant qu’ils n’auront pas libéré de place. Rien n’est effacé.',
      'Appliquer ' + mib(value),
      apply,
      { list: blocked.map(function (id) { return [id, mib(SPACES[id].used) + ' utilisés']; }) }
    );
  });
  $('quota-unlimited').addEventListener('click', function () {
    confirmAction(
      'Retirer le plafond par espace ?',
      'Seul le total du relais (1,0 Gio) limitera alors chaque espace : un espace pourrait le remplir ' +
        'pour les autres. Les espaces qui ont un quota propre le gardent.',
      'Retirer le plafond',
      function () {
        setDefaultQuota(null);
        quota.value = '';
        previewQuota();
        say('Plus de plafond par espace');
      }
    );
  });
  $('quota-reset').addEventListener('click', function () {
    setDefaultQuota(null);
    say('Quota par défaut : retour à la configuration du relais');
  });

  // ----- fermer toutes les sessions : action de protection, session seule -----
  all('[data-close-all]').forEach(function (button) {
    button.addEventListener('click', function () {
      closeSession();
      confirmAction(
        'Fermer toutes les sessions ?',
        'Toutes les sessions ouvertes sur ce panneau se ferment, celle-ci comprise : tu devras te reconnecter avec ta clé d’accès. ' +
          'À faire si tu as oublié de te déconnecter ailleurs, ou si tu soupçonnes un accès.',
        'Fermer les sessions',
        function () {
          window.location.href = 'etats.html#sessions-fermees';
        },
        { tone: '' }
      );
    });
  });

  // ----- déconnexion : l'écran de connexion est sur la planche d'états -----
  $('logout').addEventListener('click', function () {
    $('token-box').hidden = true;
    window.location.href = 'etats.html#connexion';
  });

  // =====================================================================================
  // Desktop : raccourcis clavier. Jamais pour une action qui retire, supprime ou invite :
  // celles-ci passent toujours par un bouton, un récapitulatif et la clé d'accès.
  // =====================================================================================
  var shortcuts = $('shortcuts');
  var pendingG = null;

  function openShortcuts() {
    returnFocus = document.activeElement;
    shortcuts.showModal();
    $('sc-close').focus();
  }

  $('shortcuts-open').addEventListener('click', openShortcuts);
  $('sc-close').addEventListener('click', function () {
    shortcuts.close();
  });
  shortcuts.addEventListener('close', restoreFocus);

  function rows() {
    return all('#spaces-body tr');
  }

  function kbRow() {
    return document.querySelector('#spaces-body tr.kb');
  }

  function moveRow(step) {
    var list = rows();
    if (!list.length) {
      return;
    }
    var from = kbRow() || document.querySelector('#spaces-body tr.selected');
    var index = from ? list.indexOf(from) + step : (step > 0 ? 0 : list.length - 1);
    index = Math.max(0, Math.min(list.length - 1, index));
    list.forEach(function (tr, i) {
      tr.classList.toggle('kb', i === index);
    });
    list[index].scrollIntoView({ block: 'nearest' });
    // Détail déjà ouvert : il suit la ligne, pour parcourir les espaces sans souris.
    if (lastSpace) {
      window.location.hash = '#espaces/' + list[index].getAttribute('data-space');
    }
  }

  function typing(target) {
    return target.closest && target.closest('input, textarea, select, [contenteditable="true"]');
  }

  document.addEventListener('keydown', function (event) {
    if (event.defaultPrevented || event.ctrlKey || event.metaKey || event.altKey || typing(event.target)) {
      return;
    }
    if (document.querySelector('dialog[open]') || panel.classList.contains('open')) {
      return; // Échap et Tab appartiennent à la boîte ouverte.
    }
    var key = event.key;
    if (pendingG) {
      clearTimeout(pendingG);
      pendingG = null;
      var link = document.querySelector('.nav a[data-key="' + key.toLowerCase() + '"]');
      if (link) {
        event.preventDefault();
        window.location.hash = link.getAttribute('href');
      }
      return;
    }
    var inSpaces = !$('view-espaces').hidden;
    if (key === '?') {
      event.preventDefault();
      openShortcuts();
    } else if (key === 'g' || key === 'G') {
      pendingG = setTimeout(function () { pendingG = null; }, 1200);
    } else if (key === 'r' || key === 'R') {
      event.preventDefault();
      refresh();
    } else if (inSpaces && (key === 'j' || (key === 'ArrowDown' && !event.target.closest('.detail')))) {
      event.preventDefault();
      moveRow(1);
    } else if (inSpaces && (key === 'k' || (key === 'ArrowUp' && !event.target.closest('.detail')))) {
      event.preventDefault();
      moveRow(-1);
    } else if (inSpaces && key === 'Enter' && kbRow() && !event.target.closest('a, button, summary')) {
      event.preventDefault();
      window.location.hash = '#espaces/' + kbRow().getAttribute('data-space');
    } else if ((key === 'c' || key === 'C') && lastSpace) {
      say('Identifiant ' + lastSpace + ' copié');
    } else if (key === 'Escape' && lastSpace && desktop.matches) {
      var was = lastSpace;
      window.location.hash = '#espaces';
      var row = document.querySelector('#spaces-body tr[data-space="' + was + '"]');
      if (row) {
        row.classList.add('kb');
        row.querySelector('.space-link').focus();
      }
    }
  });

  // La souris reprend la main : plus de ligne marquée au clavier.
  $('spaces-body').addEventListener('mousemove', function () {
    var row = kbRow();
    if (row) {
      row.classList.remove('kb');
    }
  });

  renderSpaces();
  renderInvitations();
  route();
})();
