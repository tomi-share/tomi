// Maquette du panneau d'administration du relais Tomi : comportement de démonstration seulement.
// Aucune requête réseau, aucune dépendance. Rien ici n'est du code de production : la navigation,
// le thème, les replis et les retours d'action sont simulés pour pouvoir juger l'interface.
// Sert à index.html (prototype navigable) et à etats.html (planche d'états : thème, « ? », copie, QR).
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

  // Le reste ne concerne que le prototype navigable (index.html).
  if (!$('spaces-layout')) {
    return;
  }

  // ----- données fictives du détail d'un espace (mêmes chiffres que le tableau) -----
  var SPACES = {
    '794e87f1': {
      badges: [['Normal', 'good']],
      dev: ['3 actifs sur 4', '1 présent en ce moment'],
      items: ['128 éléments', '14 fichiers chiffrés'],
      events: ['312 événements', 'retenus 45 j au plus'],
      created: ['il y a 2 mois', '12/09/2026 à 00:57'],
      usage: ['183 Mio', '256 Mio', 71.5, ''],
      quota: 'Quota par défaut',
      devices: [
        ['LAPTOP-XPS', 'computer', 'Windows · 0.5.1 · il y a 12 min', 'Présent', 'good', 'il y a 2 mois', 'Sans notifications', '7c3e 91af 0b52 d4e8'],
        ['Galaxy S23', 'phone', 'Android · 0.5.1 · il y a 2 h', 'Actif', '', 'il y a 2 mois', 'Activées', 'a10b 44f2 9e7c 3d05'],
        ['iPhone 15', 'phone', 'iOS · 0.5.0 · hier', 'Actif', '', 'il y a 3 sem.', 'Activées', '5d92 c8e1 07ab 6f34'],
        ['PC-Bureau', 'computer', 'Windows · 0.4.2 · il y a 2 mois', 'Inactif', 'muted', 'il y a 2 mois', 'Sans notifications', 'e4f0 12cd 88a3 5b79']
      ]
    },
    '2de8c1b2': {
      badges: [['Normal', 'good']],
      dev: ['1 actif sur 1', '0 présent en ce moment'],
      items: ['2 éléments', '0 fichier chiffré'],
      events: ['11 événements', 'retenus 45 j au plus'],
      created: ['hier', '10/11/2026 à 09:31'],
      usage: ['1,2 Mio', '256 Mio', 0.5, ''],
      quota: 'Quota par défaut',
      devices: [
        ['Pixel 8', 'phone', 'Android · 0.5.1 · il y a 2 h', 'Actif', '', 'hier', 'Activées', '31c7 fa20 6d4e 9b18']
      ]
    },
    '380a4493': {
      badges: [['Quota à 94 %', 'warn']],
      dev: ['2 actifs sur 3', '0 présent en ce moment'],
      items: ['37 éléments', '61 fichiers chiffrés'],
      events: ['96 événements', 'retenus 45 j au plus'],
      created: ['il y a 4 mois', '03/07/2026 à 14:20'],
      usage: ['241 Mio', '256 Mio', 94.1, 'warn'],
      quota: 'Quota par défaut · au-delà, les envois de cet espace sont refusés',
      devices: [
        ['ThinkPad-T14', 'computer', 'Windows · 0.5.0 · avant-hier', 'Actif', '', 'il y a 4 mois', 'Sans notifications', '9a6b 0e53 c1f7 24d8'],
        ['iPad', 'phone', 'iOS · 0.5.0 · il y a 6 j', 'Actif', '', 'il y a 3 mois', 'Activées', 'b2d4 7719 e06c 3a5f'],
        ['serveur-sauvegarde', 'terminal', 'CLI · 0.3.0 · il y a 3 mois', 'Inactif', 'muted', 'il y a 4 mois', 'Sans notifications', '0f8e 63b1 d7a2 4c90']
      ]
    }
  };

  var COMMANDS = [
    ['État détaillé de l’espace', 'tomi-relay admin status --space ID'],
    ['Appareils, avec leur identifiant complet', 'tomi-relay admin devices --space ID'],
    ['Quota propre à l’espace (OCTETS, none ou default)', 'tomi-relay admin quota --space ID OCTETS'],
    ['Retirer un appareil (ID_APPAREIL : voir la commande devices)', 'tomi-relay admin revoke --space ID ID_APPAREIL'],
    ['Supprimer l’espace', 'tomi-relay admin reset --space ID']
  ];

  function pair(node, values) {
    node.textContent = values[0];
    var sub = document.createElement('span');
    sub.className = 'sub';
    sub.textContent = values[1];
    node.appendChild(sub);
  }

  function fillDetail(id) {
    var s = SPACES[id];
    $('d-id').textContent = id;
    $('t-space').textContent = id;
    $('d-badges').innerHTML = s.badges.map(function (b) {
      return '<span class="badge ' + b[1] + '">' + b[0] + '</span>';
    }).join('');
    pair($('d-dev'), s.dev);
    pair($('d-items'), s.items);
    pair($('d-events'), s.events);
    pair($('d-created'), s.created);
    var pct = String(s.usage[2]).replace('.', ',') + ' %';
    $('d-progress').value = s.usage[2];
    $('d-progress').className = s.usage[3];
    $('d-progress').setAttribute('aria-label', 'Quota utilisé : ' + pct);
    $('d-usage').innerHTML = '<strong>' + s.usage[0] + '</strong> sur ' + s.usage[1];
    $('d-pct').textContent = pct;
    $('d-quota').textContent = s.quota;
    $('d-rotation').innerHTML = icon('check') + '<span>Changement de clé : rien en attente</span>';
    $('d-count').textContent = s.devices.length;

    var list = $('d-devices');
    list.innerHTML = '';
    s.devices.forEach(function (d) {
      var li = $('tpl-device').content.cloneNode(true);
      li.querySelector('summary use').setAttribute('href', '#i-' + d[1]);
      li.querySelector('.name').textContent = d[0];
      li.querySelector('.sub').textContent = d[2];
      li.querySelector('.badge').textContent = d[3];
      li.querySelector('.badge').className = 'badge ' + d[4];
      li.querySelector('.f-added').textContent = d[5];
      li.querySelector('.f-push').textContent = d[6];
      li.querySelector('.f-fp').textContent = d[7];
      li.querySelector('[data-copy]').setAttribute('aria-label', 'Copier l’empreinte de ' + d[0]);
      list.appendChild(li);
    });

    $('d-commands').innerHTML = COMMANDS.map(function (c) {
      return '<li><span class="what">' + c[0] + '</span><div class="cmd"><code>' + c[1].replace('ID', id) +
        '</code><button type="button" class="icon-btn" data-copy="Commande" aria-label="Copier la commande : ' +
        c[0] + '">' + icon('copy') + '</button></div></li>';
    }).join('');
  }

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

    if (space) {
      fillDetail(space);
    }
    $('space-detail').hidden = !space;
    all('#spaces-table tbody tr').forEach(function (tr) {
      tr.classList.toggle('selected', tr.getAttribute('data-space') === space);
    });
    document.body.classList.toggle('has-detail', !!space);
    $('spaces-layout').classList.toggle('has-detail', !!space);

    closeSession();
    // Sur téléphone, le détail est une page : on repart du haut. Sur desktop, la liste ne bouge pas.
    if (name !== lastView || (space !== lastSpace && !desktop.matches)) {
      window.scrollTo(0, 0);
    }
    if (space && space !== lastSpace && desktop.matches) {
      $('h-detail').focus({ preventScroll: true });
    }
    lastView = name;
    lastSpace = space;
  }

  window.addEventListener('hashchange', route);
  $('back').addEventListener('click', function () {
    window.location.hash = '#espaces';
  });

  // Desktop : toute la ligne d'un espace ouvre son détail.
  all('#spaces-table tbody tr').forEach(function (tr) {
    tr.addEventListener('click', function (event) {
      if (!event.target.closest('a, button')) {
        window.location.hash = '#espaces/' + tr.getAttribute('data-space');
      }
    });
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
  $('refresh').addEventListener('click', function () {
    var button = $('refresh');
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
  });

  // ----- confirmation : Annuler a le focus, le bouton dit ce qu'il fait -----
  var dialog = $('confirm-dialog');
  var onConfirm = null;

  function confirmAction(title, text, okLabel, then) {
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = okLabel;
    onConfirm = then;
    dialog.showModal();
    $('confirm-cancel').focus();
  }

  $('confirm-cancel').addEventListener('click', function () {
    dialog.close();
  });
  $('confirm-ok').addEventListener('click', function () {
    dialog.close();
    if (onConfirm) {
      onConfirm();
    }
  });

  // ----- invitations : le jeton n'apparaît qu'à la création -----
  function showToken() {
    $('token-box').hidden = false;
    window.scrollTo(0, 0);
    $('token-title').focus({ preventScroll: true });
    say('Invitation créée');
  }

  $('invite-new').addEventListener('click', showToken);
  $('invite-rotate').addEventListener('click', function () {
    confirmAction(
      'Remplacer les invitations en attente ?',
      'L’invitation en attente cessera de fonctionner : un jeton déjà donné ne permettra plus de créer ' +
        'd’espace. Une nouvelle invitation sera créée, et son jeton affiché une seule fois. Les espaces ' +
        'existants ne changent pas.',
      'Remplacer',
      showToken
    );
  });
  $('token-hide').addEventListener('click', function () {
    $('token-box').hidden = true;
    $('invite-new').focus();
  });

  // ----- quota : aperçu de la saisie -----
  var quota = $('quota-mib');

  function quotaLabel() {
    var text = quota.value.trim();
    if (!/^[0-9]{1,9}$/.test(text) || Number(text) < 1) {
      return null;
    }
    var mib = Number(text);
    if (mib < 1024) {
      return mib + ' Mio';
    }
    return (mib / 1024).toFixed(mib >= 102400 ? 0 : 1).replace('.', ',') + ' Gio';
  }

  function previewQuota() {
    var out = $('quota-preview');
    var label = quotaLabel();
    out.classList.toggle('bad', !!quota.value.trim() && label === null);
    if (!quota.value.trim()) {
      out.textContent = '';
    } else {
      out.textContent = label === null ? 'Un nombre entier de Mio, au moins 1' : '= ' + label + ' par espace';
    }
  }

  quota.addEventListener('input', previewQuota);
  $('quota-form').addEventListener('submit', function (event) {
    event.preventDefault();
    var label = quotaLabel();
    if (label === null) {
      say('Saisis un nombre entier de Mio (au moins 1)', 'error');
      quota.focus();
      return;
    }
    $('quota-now').textContent = label + ' par espace';
    say('Quota par défaut enregistré : ' + label + ' par espace');
  });
  $('quota-unlimited').addEventListener('click', function () {
    confirmAction(
      'Retirer le plafond par espace ?',
      'Seul le total du relais (1,0 Gio) limitera alors chaque espace : un espace pourrait le remplir ' +
        'pour les autres. Les espaces qui ont un quota propre le gardent.',
      'Retirer le plafond',
      function () {
        $('quota-now').textContent = 'Aucune limite par espace';
        quota.value = '';
        previewQuota();
        say('Plus de plafond par espace');
      }
    );
  });
  $('quota-reset').addEventListener('click', function () {
    say('Quota par défaut : retour à la configuration du relais');
  });

  // ----- déconnexion : l'écran de connexion est sur la planche d'états -----
  $('logout').addEventListener('click', function () {
    $('token-box').hidden = true;
    window.location.href = 'etats.html#connexion';
  });

  route();
})();
