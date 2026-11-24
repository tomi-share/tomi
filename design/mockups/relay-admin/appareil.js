// Mock-up only: force the device the prototype is shown on (Auto / Phone / Tablet / Desktop).
// The layout is driven by media queries on the viewport width, which a class cannot force. A forced device
// therefore shows index.html inside an <iframe> of the chosen width, so the real breakpoints apply.
// Not part of the panel: nothing here is ported to apps/relay. No network, no inline style attribute
// (sizes go through CSSOM custom properties, which the panel CSP allows).
'use strict';

(function () {
  var KEY = 'tomi-maquette-appareil';
  var DEVICES = {
    auto: { label: 'Auto' },
    phone: { label: 'Téléphone', w: 390, h: 844, hint: '390 × 844' },
    tablet: { label: 'Tablette', w: 820, h: 1180, hint: '820 × 1180' },
    desktop: { label: 'Bureau', w: 1280, h: 800, hint: '1280 × 800' }
  };
  var ORDER = ['auto', 'phone', 'tablet', 'desktop'];

  // Inside the frame: no control, the page behaves normally.
  if (/[?&]cadre=1(&|$)/.test(window.location.search)) {
    return;
  }

  var root = document.documentElement;
  var current = 'auto';
  var stage = null;
  var frame = null;
  var scaler = null;
  var shell = null;
  var picker = null;
  var launcher = null;

  function read() {
    try {
      var v = window.localStorage.getItem(KEY);
      return DEVICES[v] ? v : 'auto';
    } catch (e) {
      return 'auto';
    }
  }

  function write(v) {
    try {
      window.localStorage.setItem(KEY, v);
    } catch (e) {
      /* storage can be blocked: the choice then lasts until reload */
    }
  }

  function make(tag, cls, text) {
    var el = document.createElement(tag);
    if (cls) {
      el.className = cls;
    }
    if (text) {
      el.textContent = text;
    }
    return el;
  }

  // Segmented control shared by the toolbar (forced mode) and the popover (auto mode).
  function segmented(name) {
    var group = make('div', 'ap-seg');
    group.setAttribute('role', 'radiogroup');
    group.setAttribute('aria-label', 'Appareil de la maquette');
    ORDER.forEach(function (id) {
      var label = make('label', 'ap-opt');
      var input = document.createElement('input');
      input.type = 'radio';
      input.name = name;
      input.value = id;
      input.checked = id === current;
      input.addEventListener('change', function () {
        choose(id);
      });
      label.appendChild(input);
      label.appendChild(make('span', '', DEVICES[id].label));
      group.appendChild(label);
    });
    return group;
  }

  function fit() {
    if (!stage || current === 'auto') {
      return;
    }
    var d = DEVICES[current];
    var chrome = current === 'phone' || current === 'tablet' ? 24 : 2;
    var availW = Math.max(240, window.innerWidth - 32 - chrome);
    var availH = Math.max(240, window.innerHeight - 96 - chrome);
    var s = Math.min(1, availW / d.w, availH / d.h);
    shell.style.setProperty('--ap-w', d.w + 'px');
    shell.style.setProperty('--ap-h', d.h + 'px');
    shell.style.setProperty('--ap-s', String(s));
    var note = stage.querySelector('.ap-note');
    note.textContent = d.hint + (s < 1 ? ' · affiché à ' + Math.round(s * 100) + ' %' : '');
  }

  function buildStage() {
    stage = make('div', 'ap-stage');
    var bar = make('div', 'ap-bar');
    bar.appendChild(make('strong', 'ap-title', 'Maquette'));
    bar.appendChild(segmented('ap-device-bar'));
    bar.appendChild(make('span', 'ap-note'));
    stage.appendChild(bar);

    var body = make('div', 'ap-body');
    shell = make('div', 'ap-shell');
    scaler = make('div', 'ap-scaler');
    frame = document.createElement('iframe');
    frame.className = 'ap-frame';
    frame.title = 'Aperçu de la maquette';
    scaler.appendChild(frame);
    shell.appendChild(scaler);
    body.appendChild(shell);
    stage.appendChild(body);
    document.body.appendChild(stage);
  }

  function buildLauncher() {
    launcher = make('div', 'ap-launch');
    var btn = make('button', 'ap-fab', 'Appareil');
    btn.type = 'button';
    btn.setAttribute('aria-expanded', 'false');
    picker = make('div', 'ap-pop');
    picker.hidden = true;
    picker.appendChild(make('p', 'ap-pop-t', 'Forcer l’appareil'));
    picker.appendChild(segmented('ap-device-pop'));
    picker.appendChild(make('p', 'ap-pop-h', 'Téléphone, tablette et bureau affichent la maquette dans un cadre de cette largeur : les vrais points de bascule s’appliquent.'));
    btn.addEventListener('click', function () {
      var open = picker.hidden;
      picker.hidden = !open;
      btn.setAttribute('aria-expanded', open ? 'true' : 'false');
    });
    document.addEventListener('keydown', function (e) {
      if (e.key === 'Escape' && !picker.hidden) {
        picker.hidden = true;
        btn.setAttribute('aria-expanded', 'false');
        btn.focus();
      }
    });
    launcher.appendChild(picker);
    launcher.appendChild(btn);
    document.body.appendChild(launcher);
  }

  function choose(id) {
    current = id;
    write(id);
    apply();
  }

  function apply() {
    var forced = current !== 'auto';
    root.classList.toggle('ap-forced', forced);
    if (!stage) {
      buildStage();
    }
    stage.hidden = !forced;
    launcher.hidden = forced;
    if (picker) {
      picker.hidden = true;
      launcher.querySelector('.ap-fab').setAttribute('aria-expanded', 'false');
    }
    // keep every radio group in sync with the choice
    Array.prototype.forEach.call(document.querySelectorAll('.ap-seg input'), function (input) {
      input.checked = input.value === current;
    });
    shell.className = 'ap-shell ap-' + (forced ? current : 'phone');
    if (forced) {
      var d = DEVICES[current];
      var src = 'index.html?cadre=1' + window.location.hash;
      if (frame.getAttribute('data-device') !== current) {
        frame.setAttribute('data-device', current);
        frame.setAttribute('src', src);
      }
      frame.setAttribute('width', String(d.w));
      frame.setAttribute('height', String(d.h));
      fit();
    } else {
      frame.removeAttribute('data-device');
      frame.removeAttribute('src');
    }
  }

  function start() {
    buildStage();
    buildLauncher();
    current = read();
    apply();
  }

  window.addEventListener('resize', fit);
  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
