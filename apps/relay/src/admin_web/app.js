// Administration page of the Tomi relay.
// No dependency, no framework, no external resource. All text coming from the server
// is inserted with textContent, never interpreted as HTML.
'use strict';

(function () {
  var VIEWS = ['loading', 'nojs', 'enroll', 'login', 'dashboard'];
  var FOCUS_TARGET = {
    loading: 'h-loading',
    nojs: 'h-unsupported',
    enroll: 'h-enroll',
    login: 'h-login',
    dashboard: 'h-status'
  };

  function $(id) {
    return document.getElementById(id);
  }

  // ----- base64url <-> bytes ------------------------------------------------------------

  function fromB64u(text) {
    var s = String(text).replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4 !== 0) {
      s += '=';
    }
    var bin = atob(s);
    var out = new Uint8Array(bin.length);
    for (var i = 0; i < bin.length; i++) {
      out[i] = bin.charCodeAt(i);
    }
    return out;
  }

  function toB64u(buffer) {
    var bytes = new Uint8Array(buffer);
    var bin = '';
    for (var i = 0; i < bytes.length; i++) {
      bin += String.fromCharCode(bytes[i]);
    }
    return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  // ----- QR code: hand-written encoder -----------------------------------------------
  // No library, no external resource, no network call. Byte mode, error correction
  // level M, versions 1 to 40 (the smallest one that holds the text). The mask is chosen by
  // the standard penalty score (the four rules of ISO/IEC 18004). Only qrMatrix() is
  // used by the page; it returns the module matrix (true = dark module).

  // Error-correction bytes per block and number of blocks, level M, indexed by version (0 unused).
  var QR_ECC_PER_BLOCK_M = [-1, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26,
    26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28];
  var QR_BLOCKS_M = [-1, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14,
    16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49];

  // Galois field GF(256), reducing polynomial x^8 + x^4 + x^3 + x^2 + 1 (0x11D), generator 2.
  var GF_EXP = [];
  var GF_LOG = [];
  (function () {
    var x = 1;
    var i;
    for (i = 0; i < 255; i++) {
      GF_EXP[i] = x;
      GF_LOG[x] = i;
      x <<= 1;
      if (x & 0x100) {
        x ^= 0x11d;
      }
    }
    for (i = 255; i < 512; i++) {
      GF_EXP[i] = GF_EXP[i - 255];
    }
  })();

  function gfMul(a, b) {
    if (a === 0 || b === 0) {
      return 0;
    }
    return GF_EXP[GF_LOG[a] + GF_LOG[b]];
  }

  // Generator polynomial of degree d, (x - 2^0)(x - 2^1)...(x - 2^(d-1)), without the leading coefficient
  // (always 1), from the highest-degree term down to the constant term.
  function rsDivisor(degree) {
    var result = [];
    var i;
    var j;
    for (i = 0; i < degree - 1; i++) {
      result.push(0);
    }
    result.push(1);
    var root = 1;
    for (i = 0; i < degree; i++) {
      for (j = 0; j < result.length; j++) {
        result[j] = gfMul(result[j], root);
        if (j + 1 < result.length) {
          result[j] ^= result[j + 1];
        }
      }
      root = gfMul(root, 2);
    }
    return result;
  }

  // Remainder of the message (x^d) divided by the generator polynomial: the error-correction bytes.
  function rsRemainder(data, divisor) {
    var result = [];
    var i;
    for (i = 0; i < divisor.length; i++) {
      result.push(0);
    }
    data.forEach(function (b) {
      var factor = b ^ result.shift();
      result.push(0);
      for (i = 0; i < divisor.length; i++) {
        result[i] ^= gfMul(divisor[i], factor);
      }
    });
    return result;
  }

  // Number of modules that carry data (everything except the function patterns), versions 1..40.
  function qrRawModules(ver) {
    var result = (16 * ver + 128) * ver + 64;
    if (ver >= 2) {
      var align = Math.floor(ver / 7) + 2;
      result -= (25 * align - 10) * align - 55;
      if (ver >= 7) {
        result -= 36;
      }
    }
    return result;
  }

  function qrDataCodewords(ver) {
    return Math.floor(qrRawModules(ver) / 8) - QR_ECC_PER_BLOCK_M[ver] * QR_BLOCKS_M[ver];
  }

  // Coordinates (rows and columns) of the centres of the alignment patterns.
  function qrAlignPositions(ver) {
    if (ver === 1) {
      return [];
    }
    var count = Math.floor(ver / 7) + 2;
    var step = ver === 32 ? 26 : Math.ceil((ver * 4 + 4) / (count * 2 - 2)) * 2;
    var result = [6];
    for (var pos = ver * 4 + 10; result.length < count; pos -= step) {
      result.splice(1, 0, pos);
    }
    return result;
  }

  // Data + error-correction bytes, split into blocks then interleaved as the standard requires.
  function qrInterleave(data, ver) {
    var numBlocks = QR_BLOCKS_M[ver];
    var eccLen = QR_ECC_PER_BLOCK_M[ver];
    var rawCodewords = Math.floor(qrRawModules(ver) / 8);
    var numShort = numBlocks - (rawCodewords % numBlocks);
    var shortLen = Math.floor(rawCodewords / numBlocks);
    var divisor = rsDivisor(eccLen);
    var blocks = [];
    var k = 0;
    var i;
    var j;
    for (i = 0; i < numBlocks; i++) {
      var dat = data.slice(k, k + shortLen - eccLen + (i < numShort ? 0 : 1));
      k += dat.length;
      var ecc = rsRemainder(dat, divisor);
      if (i < numShort) {
        dat.push(0); // empty slot: short blocks do not have this last data byte
      }
      blocks.push(dat.concat(ecc));
    }
    var out = [];
    for (i = 0; i < blocks[0].length; i++) {
      for (j = 0; j < numBlocks; j++) {
        if (i !== shortLen - eccLen || j >= numShort) {
          out.push(blocks[j][i]);
        }
      }
    }
    return out;
  }

  function qrBit(value, index) {
    return ((value >>> index) & 1) !== 0;
  }

  // Builds the symbol: function patterns, byte placement, mask (0..7) or choice of the
  // best mask if `forcedMask` is -1. Returns { matrix, mask }.
  function qrBuild(ver, codewords, forcedMask) {
    var size = ver * 4 + 17;
    var mod = [];
    var fn = [];
    var x;
    var y;
    var i;
    for (y = 0; y < size; y++) {
      var r = [];
      var f = [];
      for (x = 0; x < size; x++) {
        r.push(false);
        f.push(false);
      }
      mod.push(r);
      fn.push(f);
    }

    function setFn(px, py, dark) {
      mod[py][px] = dark;
      fn[py][px] = true;
    }

    function finder(cx, cy) {
      for (var dy = -4; dy <= 4; dy++) {
        for (var dx = -4; dx <= 4; dx++) {
          var dist = Math.max(Math.abs(dx), Math.abs(dy));
          var xx = cx + dx;
          var yy = cy + dy;
          if (xx >= 0 && xx < size && yy >= 0 && yy < size) {
            setFn(xx, yy, dist !== 2 && dist !== 4);
          }
        }
      }
    }

    function alignment(cx, cy) {
      for (var dy = -2; dy <= 2; dy++) {
        for (var dx = -2; dx <= 2; dx++) {
          setFn(cx + dx, cy + dy, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
        }
      }
    }

    // Level M = bits 00. 5 format bits (level, mask), 10 BCH bits (0x537), XOR 0x5412.
    function formatBits(mask) {
      var data = mask; // (00 << 3) | mask
      var rem = data;
      for (var n = 0; n < 10; n++) {
        rem = (rem << 1) ^ ((rem >>> 9) * 0x537);
      }
      var bits = ((data << 10) | rem) ^ 0x5412;
      for (n = 0; n <= 5; n++) {
        setFn(8, n, qrBit(bits, n));
      }
      setFn(8, 7, qrBit(bits, 6));
      setFn(8, 8, qrBit(bits, 7));
      setFn(7, 8, qrBit(bits, 8));
      for (n = 9; n < 15; n++) {
        setFn(14 - n, 8, qrBit(bits, n));
      }
      for (n = 0; n < 8; n++) {
        setFn(size - 1 - n, 8, qrBit(bits, n));
      }
      for (n = 8; n < 15; n++) {
        setFn(8, size - 15 + n, qrBit(bits, n));
      }
      setFn(8, size - 8, true); // the fixed dark module
    }

    // Function patterns: timing, finder, alignment, version and format areas.
    for (i = 0; i < size; i++) {
      setFn(6, i, i % 2 === 0);
      setFn(i, 6, i % 2 === 0);
    }
    finder(3, 3);
    finder(size - 4, 3);
    finder(3, size - 4);
    var pos = qrAlignPositions(ver);
    var last = pos.length - 1;
    for (i = 0; i <= last; i++) {
      for (var j = 0; j <= last; j++) {
        if (!((i === 0 && j === 0) || (i === 0 && j === last) || (i === last && j === 0))) {
          alignment(pos[i], pos[j]);
        }
      }
    }
    formatBits(0); // reserves the space; rewritten with the real mask below
    if (ver >= 7) {
      var vrem = ver;
      for (i = 0; i < 12; i++) {
        vrem = (vrem << 1) ^ ((vrem >>> 11) * 0x1f25);
      }
      var vbits = (ver << 12) | vrem;
      for (i = 0; i < 18; i++) {
        var a = size - 11 + (i % 3);
        var b = Math.floor(i / 3);
        setFn(a, b, qrBit(vbits, i));
        setFn(b, a, qrBit(vbits, i));
      }
    }

    // Bit placement, in two-column strips, bottom to top then top to bottom,
    // skipping the timing column (x = 6).
    var bitIndex = 0;
    for (var right = size - 1; right >= 1; right -= 2) {
      if (right === 6) {
        right = 5;
      }
      for (var vert = 0; vert < size; vert++) {
        for (var c = 0; c < 2; c++) {
          var px = right - c;
          var upward = ((right + 1) & 2) === 0;
          var py = upward ? size - 1 - vert : vert;
          if (!fn[py][px] && bitIndex < codewords.length * 8) {
            mod[py][px] = qrBit(codewords[bitIndex >>> 3], 7 - (bitIndex & 7));
            bitIndex++;
          }
        }
      }
    }

    function applyMask(mask) {
      for (var yy = 0; yy < size; yy++) {
        for (var xx = 0; xx < size; xx++) {
          var invert;
          switch (mask) {
            case 0: invert = (xx + yy) % 2 === 0; break;
            case 1: invert = yy % 2 === 0; break;
            case 2: invert = xx % 3 === 0; break;
            case 3: invert = (xx + yy) % 3 === 0; break;
            case 4: invert = (Math.floor(xx / 3) + Math.floor(yy / 2)) % 2 === 0; break;
            case 5: invert = ((xx * yy) % 2) + ((xx * yy) % 3) === 0; break;
            case 6: invert = (((xx * yy) % 2) + ((xx * yy) % 3)) % 2 === 0; break;
            default: invert = (((xx + yy) % 2) + ((xx * yy) % 3)) % 2 === 0; break;
          }
          if (invert && !fn[yy][xx]) {
            mod[yy][xx] = !mod[yy][xx];
          }
        }
      }
    }

    var best = forcedMask;
    if (best < 0) {
      var bestScore = Infinity;
      for (var m = 0; m < 8; m++) {
        applyMask(m);
        formatBits(m);
        var score = qrPenalty(mod, size);
        if (score < bestScore) {
          bestScore = score;
          best = m;
        }
        applyMask(m); // the mask is an involution: applying it again removes it
      }
    }
    applyMask(best);
    formatBits(best);
    return { matrix: mod, mask: best };
  }

  // The four penalty rules of the standard (ISO/IEC 18004, 7.8.3).
  function qrPenalty(mod, size) {
    var result = 0;
    var P1 = [1, 0, 1, 1, 1, 0, 1, 0, 0, 0, 0];
    var P2 = [0, 0, 0, 0, 1, 0, 1, 1, 1, 0, 1];
    var line;
    var i;
    var k;
    var x;
    var y;

    // Rules 1 (runs of 5 or more modules of the same colour: 3 + (n - 5)) and 3 (patterns
    // dark-light-dark-dark-dark-light-dark surrounded by 4 light modules: 40), rows then columns.
    for (var dir = 0; dir < 2; dir++) {
      var get = dir === 0
        ? function (a, b) { return mod[a][b]; }
        : function (a, b) { return mod[b][a]; };
      for (line = 0; line < size; line++) {
        var run = 1;
        for (i = 1; i < size; i++) {
          if (get(line, i) === get(line, i - 1)) {
            run++;
          } else {
            if (run >= 5) {
              result += run - 2;
            }
            run = 1;
          }
        }
        if (run >= 5) {
          result += run - 2;
        }
        for (i = 0; i + 11 <= size; i++) {
          var m1 = true;
          var m2 = true;
          for (k = 0; k < 11; k++) {
            var v = get(line, i + k);
            if (v !== (P1[k] === 1)) {
              m1 = false;
            }
            if (v !== (P2[k] === 1)) {
              m2 = false;
            }
          }
          if (m1 || m2) {
            result += 40;
          }
        }
      }
    }

    // Rule 2: blocks of 2 x 2 modules of the same colour: 3 each.
    for (y = 0; y < size - 1; y++) {
      for (x = 0; x < size - 1; x++) {
        var c = mod[y][x];
        if (c === mod[y][x + 1] && c === mod[y + 1][x] && c === mod[y + 1][x + 1]) {
          result += 3;
        }
      }
    }

    // Rule 4: 10 points per 5% step of deviation between the share of dark modules and 50%.
    var dark = 0;
    for (y = 0; y < size; y++) {
      for (x = 0; x < size; x++) {
        if (mod[y][x]) {
          dark++;
        }
      }
    }
    var total = size * size;
    result += Math.floor(Math.abs(dark * 20 - total * 10) / total) * 10;
    return result;
  }

  // Encodes `text` (UTF-8, byte mode, correction M). `forcedMask` (0..7) is reserved for
  // checks; without it, the mask with the lowest penalty is chosen.
  // Returns { matrix, version, mask }; throws if the text fits in no version.
  function qrEncode(text, forcedMask) {
    if (typeof TextEncoder !== 'function') {
      throw new Error('qr: encodage indisponible');
    }
    var bytes = Array.prototype.slice.call(new TextEncoder().encode(String(text)));
    var ver = 1;
    while (ver <= 40 && 4 + (ver < 10 ? 8 : 16) + bytes.length * 8 > qrDataCodewords(ver) * 8) {
      ver++;
    }
    if (ver > 40) {
      throw new Error('qr: texte trop long');
    }
    var capacityBits = qrDataCodewords(ver) * 8;
    var bits = [];
    function put(value, length) {
      for (var n = length - 1; n >= 0; n--) {
        bits.push((value >>> n) & 1);
      }
    }
    put(4, 4); // byte mode: 0100
    put(bytes.length, ver < 10 ? 8 : 16);
    bytes.forEach(function (b) {
      put(b, 8);
    });
    put(0, Math.min(4, capacityBits - bits.length)); // terminator
    put(0, (8 - (bits.length % 8)) % 8);
    for (var pad = 0xec; bits.length < capacityBits; pad ^= 0xec ^ 0x11) {
      put(pad, 8);
    }
    var data = [];
    for (var i = 0; i < bits.length; i += 8) {
      var b = 0;
      for (var j = 0; j < 8; j++) {
        b = (b << 1) | bits[i + j];
      }
      data.push(b);
    }
    var built = qrBuild(ver, qrInterleave(data, ver), forcedMask === undefined ? -1 : forcedMask);
    return { matrix: built.matrix, version: ver, mask: built.mask };
  }

  function qrMatrix(text) {
    return qrEncode(text).matrix;
  }

  // ----- display -----------------------------------------------------------------------

  var toastTimer = null;

  // Action feedback: at the bottom of the screen, visible wherever you are on the page. A success
  // clears itself; an error stays until it is closed (or until the next message).
  function say(text, kind) {
    var box = $('message');
    var toast = $('toast');
    if (toastTimer) {
      clearTimeout(toastTimer);
      toastTimer = null;
    }
    if (!text) {
      box.textContent = '';
      toast.hidden = true;
      return;
    }
    toast.className = 'toast' + (kind ? ' ' + kind : '');
    // An error is announced at once, the rest politely. The area is shown before it is
    // filled: a screen reader does not announce what changes in a hidden area.
    box.setAttribute('role', kind === 'error' ? 'alert' : 'status');
    toast.hidden = false;
    box.textContent = text;
    if (kind !== 'error') {
      toastTimer = setTimeout(function () {
        say('');
      }, 8000);
    }
  }

  function show(name) {
    VIEWS.forEach(function (v) {
      $('view-' + v).hidden = v !== name;
    });
    var dashboard = name === 'dashboard';
    $('logout').hidden = !dashboard;
    $('dash-nav').hidden = !dashboard;
    $('session-info').hidden = !dashboard;
    var target = $(FOCUS_TARGET[name]);
    if (target) {
      target.focus();
    }
  }

  function clear(node) {
    while (node.firstChild) {
      node.removeChild(node.firstChild);
    }
  }

  function el(tag, className, text) {
    var e = document.createElement(tag);
    if (className) {
      e.className = className;
    }
    if (text !== undefined && text !== null) {
      e.textContent = text;
    }
    return e;
  }

  // One cell; `label` serves as the column title when the table becomes a card (mobile).
  function cell(row, content, label, header) {
    var c = document.createElement(header ? 'th' : 'td');
    if (header) {
      c.setAttribute('scope', 'row');
    }
    if (label) {
      c.setAttribute('data-label', label);
    }
    if (content instanceof Node) {
      c.appendChild(content);
    } else {
      c.textContent = content;
    }
    row.appendChild(c);
    return c;
  }

  function badge(text, kind) {
    return el('span', 'badge' + (kind ? ' ' + kind : ''), text);
  }

  function fmtBytes(n) {
    if (n === null || n === undefined) {
      return 'aucune limite';
    }
    var units = ['o', 'Kio', 'Mio', 'Gio', 'Tio'];
    var value = Number(n);
    var i = 0;
    while (value >= 1024 && i < units.length - 1) {
      value /= 1024;
      i++;
    }
    var digits = i === 0 || value >= 100 ? 0 : 1;
    return value.toFixed(digits).replace('.', ',') + ' ' + units[i];
  }

  function fmtCount(n) {
    var v = Number(n || 0);
    try {
      return new Intl.NumberFormat('fr-FR').format(v);
    } catch (e) {
      return String(v);
    }
  }

  function plural(n, one, many) {
    return fmtCount(n) + ' ' + (Number(n) > 1 ? many : one);
  }

  function percent(used, max) {
    if (!max) {
      return null;
    }
    return Math.min(100, Math.round((Number(used) / Number(max)) * 1000) / 10);
  }

  function fmtPercent(p) {
    return String(p).replace('.', ',') + ' %';
  }

  function fmtDate(text) {
    if (!text) {
      return 'jamais';
    }
    var d = new Date(text);
    if (isNaN(d.getTime())) {
      return text;
    }
    return d.toLocaleString('fr-FR');
  }

  // Offset between the server clock and the browser clock: the "3 days ago" counts are computed
  // from the server time, which wrote the dates.
  var serverOffset = 0;

  function serverNow() {
    return Date.now() + serverOffset;
  }

  var RTF = null;
  try {
    RTF = new Intl.RelativeTimeFormat('fr', { numeric: 'auto', style: 'short' });
  } catch (e) {
    RTF = null;
  }

  function rel(text) {
    if (!text) {
      return 'jamais';
    }
    var t = Date.parse(text);
    if (isNaN(t)) {
      return text;
    }
    var diff = (t - serverNow()) / 1000;
    var a = Math.abs(diff);
    if (a < 45) {
      return 'à l’instant';
    }
    var steps = [
      [3600, 60, 'minute', 'min'],
      [86400, 3600, 'hour', 'h'],
      [86400 * 30, 86400, 'day', 'j'],
      [86400 * 365, 86400 * 30, 'month', 'mois'],
      [Infinity, 86400 * 365, 'year', 'an']
    ];
    for (var i = 0; i < steps.length; i++) {
      if (a < steps[i][0]) {
        var v = Math.round(diff / steps[i][1]);
        if (RTF) {
          return RTF.format(v, steps[i][2]);
        }
        var n = Math.abs(v) + ' ' + steps[i][3];
        return v < 0 ? 'il y a ' + n : 'dans ' + n;
      }
    }
    return text;
  }

  // <time>: the relative duration on screen, the exact date in a tooltip and for screen readers.
  function timeEl(text) {
    if (!text) {
      return el('span', 'muted', 'jamais');
    }
    var t = el('time', null, rel(text));
    t.setAttribute('datetime', text);
    t.title = fmtDate(text);
    return t;
  }

  function fmtDuration(secs) {
    var s = Math.max(0, Math.round(Number(secs) || 0));
    var d = Math.floor(s / 86400);
    var h = Math.floor((s % 86400) / 3600);
    var m = Math.floor((s % 3600) / 60);
    if (d > 0) {
      return d + ' j' + (h ? ' ' + h + ' h' : '');
    }
    if (h > 0) {
      return h + ' h' + (m ? ' ' + m + ' min' : '');
    }
    if (m > 0) {
      return m + ' min';
    }
    return s + ' s';
  }

  // ----- theme (local preference of this browser: nothing secret) ----------------------

  var THEME_KEY = 'tomi-admin-theme';

  function applyTheme(value) {
    if (value === 'light' || value === 'dark') {
      document.documentElement.setAttribute('data-theme', value);
    } else {
      document.documentElement.removeAttribute('data-theme');
    }
  }

  function loadTheme() {
    var value = 'auto';
    try {
      value = window.localStorage.getItem(THEME_KEY) || 'auto';
    } catch (e) {
      value = 'auto';
    }
    if (value !== 'light' && value !== 'dark') {
      value = 'auto';
    }
    $('theme').value = value;
    applyTheme(value);
  }

  function saveTheme() {
    var value = $('theme').value;
    applyTheme(value);
    try {
      if (value === 'light' || value === 'dark') {
        window.localStorage.setItem(THEME_KEY, value);
      } else {
        window.localStorage.removeItem(THEME_KEY);
      }
    } catch (e) {
      // Storage unavailable (private browsing): the choice applies to this page only.
    }
  }

  // ----- in-page confirmation --------------------------------------------------------

  // A real confirmation window (<dialog>) rather than window.confirm: a title, the
  // consequence in plain words, and a button that says what it does. Cancel has focus by default.
  function confirmAction(title, text, okLabel) {
    var dlg = $('confirm-dialog');
    if (!dlg || typeof dlg.showModal !== 'function') {
      return Promise.resolve(window.confirm(title + '\n\n' + text));
    }
    var before = document.activeElement;
    $('confirm-title').textContent = title;
    $('confirm-text').textContent = text;
    $('confirm-ok').textContent = okLabel;
    return new Promise(function (resolve) {
      var ok = $('confirm-ok');
      var cancel = $('confirm-cancel');
      function done(answer) {
        ok.removeEventListener('click', onOk);
        cancel.removeEventListener('click', onCancel);
        dlg.removeEventListener('cancel', onCancel);
        if (dlg.open) {
          dlg.close();
        }
        if (before && typeof before.focus === 'function') {
          before.focus();
        }
        resolve(answer);
      }
      function onOk() {
        done(true);
      }
      function onCancel() {
        done(false);
      }
      ok.addEventListener('click', onOk);
      cancel.addEventListener('click', onCancel);
      dlg.addEventListener('cancel', onCancel); // Escape key
      dlg.showModal();
      cancel.focus();
    });
  }

  // ----- clipboard ---------------------------------------------------------------------

  function copyText(text, what) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).then(
        function () {
          say(what + ' copié.', 'ok');
        },
        function () {
          say('Copie impossible : sélectionne le texte et copie-le à la main.', 'error');
        }
      );
    }
    say('Copie impossible : sélectionne le texte et copie-le à la main.', 'error');
    return Promise.resolve();
  }

  function copyButton(text, what, label) {
    var b = el('button', 'secondary small', 'Copier');
    b.type = 'button';
    b.setAttribute('aria-label', label);
    b.addEventListener('click', function () {
      copyText(text, what);
    });
    return b;
  }

  // ----- server calls ---------------------------------------------------------------

  // Write requests carry the X-Tomi-Admin header; the browser adds the Origin header.
  // The session cookie is HttpOnly: this script never sees it.
  function api(method, path, body) {
    var options = {
      method: method,
      credentials: 'same-origin',
      cache: 'no-store',
      headers: { 'X-Tomi-Admin': '1' }
    };
    if (body !== undefined) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(body);
    }
    return fetch(path, options).then(function (response) {
      // The server pushes the session end back on every authenticated request, even one refused afterwards
      // (400, 404, 500): only a 401 response proves that it was not pushed back.
      if (response.status !== 401) {
        noteActivity();
      }
      if (response.status === 204) {
        return { status: 204, data: null };
      }
      return response.text().then(function (text) {
        var data = null;
        try {
          data = text ? JSON.parse(text) : null;
        } catch (e) {
          data = null;
        }
        return { status: response.status, data: data };
      });
    });
  }

  // A dashboard read: 401 = session over, otherwise 200 with the expected body.
  function read(path) {
    return api('GET', path).then(function (r) {
      if (r.status === 401) {
        throw new Error('session');
      }
      if (r.status !== 200 || !r.data) {
        throw new Error('failed');
      }
      return r.data;
    });
  }

  // ----- session --------------------------------------------------------------------------

  // Session deadlines, in browser time. Inactivity is recounted at each response
  // from the server; the 8-hour limit does not move. Nothing refreshes the page on its own: an
  // automatic refresh would keep the session open forever.
  var session = { idleTtl: 0, absoluteEnd: 0, idleEnd: 0 };
  var sessionTimer = null;

  function noteActivity() {
    if (session.idleTtl) {
      session.idleEnd = Date.now() + session.idleTtl * 1000;
    }
  }

  function setSession(about) {
    var s = about.session || {};
    session.idleTtl = Number(s.idle_ttl_secs) || 0;
    session.idleEnd = Date.parse(s.idle_ends_at) - serverOffset;
    session.absoluteEnd = Date.parse(s.absolute_ends_at) - serverOffset;
    tickSession();
    if (!sessionTimer) {
      sessionTimer = setInterval(tickSession, 20000);
    }
  }

  function stopSession() {
    if (sessionTimer) {
      clearInterval(sessionTimer);
      sessionTimer = null;
    }
    session = { idleTtl: 0, absoluteEnd: 0, idleEnd: 0 };
  }

  function tickSession() {
    var info = $('session-info');
    if (!session.idleEnd || !session.absoluteEnd) {
      info.textContent = '';
      return;
    }
    var end = Math.min(session.idleEnd, session.absoluteEnd);
    var left = Math.round((end - Date.now()) / 1000);
    if (left <= 0) {
      lostSession();
      return;
    }
    var hard = session.absoluteEnd <= session.idleEnd;
    var minutes = Math.max(1, Math.ceil(left / 60));
    info.textContent =
      'Session : ' + (minutes <= 5 ? 'se ferme dans ' : 'encore ') + fmtDuration(minutes * 60) +
      (hard ? ' (limite de 8 h)' : '');
    info.className = 'session-info' + (minutes <= 5 ? ' soon' : '');
    info.title = hard
      ? 'La session atteint sa durée maximale de 8 heures : il faudra te reconnecter.'
      : 'Toute action ou actualisation repousse la fin de session (30 minutes sans activité).';
  }

  function lostSession() {
    hideToken();
    stopSession();
    selectedSpace = null;
    say('La session a pris fin. Connecte-toi de nouveau.', 'error');
    show('login');
  }

  // ----- passkeys --------------------------------------------------------------------

  function webauthnAvailable() {
    return !!(window.PublicKeyCredential && navigator.credentials && navigator.credentials.create);
  }

  function describeFailure(err) {
    if (err && (err.name === 'NotAllowedError' || err.name === 'AbortError')) {
      return 'Opération annulée ou expirée. Réessaie.';
    }
    return 'Échec. Réessaie, ou vérifie le code.';
  }

  function enroll(event) {
    event.preventDefault();
    var code = $('code').value.trim();
    if (!code) {
      say('Saisis le code d’enrôlement.', 'error');
      $('code').focus();
      return;
    }
    var button = $('enroll-submit');
    button.disabled = true;
    say('Vérification du code…');
    api('POST', 'api/register/options', { code: code })
      .then(function (r) {
        if (r.status !== 200 || !r.data || !r.data.publicKey) {
          throw new Error('refused');
        }
        var pk = r.data.publicKey;
        pk.challenge = fromB64u(pk.challenge);
        pk.user.id = fromB64u(pk.user.id);
        say('Valide la création de la clé d’accès sur ton appareil…');
        return navigator.credentials.create({ publicKey: pk });
      })
      .then(function (credential) {
        var res = credential.response;
        return api('POST', 'api/register/finish', {
          clientDataJSON: toB64u(res.clientDataJSON),
          attestationObject: toB64u(res.attestationObject)
        });
      })
      .then(function (r) {
        if (r.status !== 204) {
          throw new Error('refused');
        }
        $('code').value = '';
        say('Clé d’accès créée. Connecte-toi maintenant.', 'ok');
        show('login');
      })
      .catch(function (err) {
        say(describeFailure(err), 'error');
        $('code').focus();
      })
      .then(function () {
        button.disabled = false;
      });
  }

  function login() {
    var button = $('login-submit');
    button.disabled = true;
    say('Préparation de la connexion…');
    api('POST', 'api/login/options', {})
      .then(function (r) {
        if (r.status !== 200 || !r.data || !r.data.publicKey) {
          throw new Error('refused');
        }
        var pk = r.data.publicKey;
        pk.challenge = fromB64u(pk.challenge);
        (pk.allowCredentials || []).forEach(function (c) {
          c.id = fromB64u(c.id);
        });
        say('Valide la connexion sur ton appareil…');
        return navigator.credentials.get({ publicKey: pk });
      })
      .then(function (credential) {
        var res = credential.response;
        return api('POST', 'api/login/finish', {
          id: toB64u(credential.rawId),
          clientDataJSON: toB64u(res.clientDataJSON),
          authenticatorData: toB64u(res.authenticatorData),
          signature: toB64u(res.signature)
        });
      })
      .then(function (r) {
        if (r.status !== 204) {
          throw new Error('refused');
        }
        say('');
        return openDashboard();
      })
      .catch(function (err) {
        say(describeFailure(err), 'error');
      })
      .then(function () {
        button.disabled = false;
      });
  }

  function logout() {
    // The page clears at once, without waiting for the server: a token does not stay displayed
    // because of a network outage.
    hideToken();
    stopSession();
    selectedSpace = null;
    api('POST', 'api/logout', {}).then(
      function () {
        say('Tu es déconnecté.', 'ok');
        show('login');
      },
      function () {
        say('Déconnexion impossible : le serveur ne répond pas. La session se fermera d’elle-même.', 'error');
        show('login');
      }
    );
  }

  // ----- dashboard data ----------------------------------------------------------

  // The last read of each source: what is needed to redraw (sorting), compute the attention points
  // and the diagnostic without asking the server again.
  var data = {
    status: null,
    spaces: [],
    devices: [],
    quota: null,
    invitations: [],
    journal: [],
    about: null
  };
  var selectedSpace = null;

  // ----- table sorting -----------------------------------------------------------------

  var sorts = {
    spaces: { key: 'created_at', dir: 1 },
    devices: { key: 'last_seen_at', dir: -1 }
  };

  function sortValue(row, key) {
    var v = row[key];
    if (key === 'members') {
      return Number(row.active || 0) * 1000 + Number(row.members || 0);
    }
    if (v === null || v === undefined) {
      return '';
    }
    return v;
  }

  function sorted(rows, sort) {
    return rows.slice().sort(function (a, b) {
      var x = sortValue(a, sort.key);
      var y = sortValue(b, sort.key);
      var c;
      if (typeof x === 'number' && typeof y === 'number') {
        c = x - y;
      } else {
        c = String(x).localeCompare(String(y), 'fr', { numeric: true, sensitivity: 'base' });
      }
      return c * sort.dir;
    });
  }

  // Each sortable header becomes a button; aria-sort states the current order.
  function setupSort(tableId, name, render) {
    var heads = $(tableId).querySelectorAll('th[data-sort]');
    Array.prototype.forEach.call(heads, function (th) {
      var key = th.getAttribute('data-sort');
      var label = th.textContent;
      clear(th);
      var b = el('button', 'sort', label);
      b.type = 'button';
      var dir = el('span', 'dir');
      dir.setAttribute('aria-hidden', 'true');
      b.appendChild(dir);
      b.addEventListener('click', function () {
        var s = sorts[name];
        if (s.key === key) {
          s.dir = -s.dir;
        } else {
          s.key = key;
          s.dir = key.slice(-3) === '_at' || key === 'used_bytes' || key === 'items' ? -1 : 1;
        }
        markSort(tableId, name);
        render();
      });
      th.appendChild(b);
    });
    markSort(tableId, name);
  }

  function markSort(tableId, name) {
    var s = sorts[name];
    var heads = $(tableId).querySelectorAll('th[data-sort]');
    Array.prototype.forEach.call(heads, function (th) {
      var on = th.getAttribute('data-sort') === s.key;
      var dir = th.querySelector('.dir');
      if (on) {
        th.setAttribute('aria-sort', s.dir > 0 ? 'ascending' : 'descending');
        dir.textContent = s.dir > 0 ? '▲' : '▼';
      } else {
        th.removeAttribute('aria-sort');
        dir.textContent = '';
      }
    });
  }

  // ----- overview ---------------------------------------------------------------------

  function fcmLabel(fcm) {
    var states = {
      disabled: 'désactivées',
      ok: 'en service',
      auth_error: 'erreur d’authentification',
      degraded: 'dégradées',
      unknown: 'pas encore utilisées'
    };
    var state = fcm && fcm.state ? fcm.state : 'unknown';
    return states[state] || state;
  }

  function tile(list, label, value, sub, extra) {
    var box = el('div', 'tile');
    box.appendChild(el('dt', null, label));
    box.appendChild(el('dd', 'value', value));
    if (extra) {
      var holder = el('dd');
      holder.appendChild(extra);
      box.appendChild(holder);
    }
    if (sub) {
      box.appendChild(el('dd', 'sub', sub));
    }
    list.appendChild(box);
  }

  function gauge(used, max, label) {
    var p = percent(used, max);
    if (p === null) {
      return null;
    }
    var g = document.createElement('progress');
    g.max = 100;
    g.value = p;
    g.className = p >= 98 ? 'full' : p >= 90 ? 'warn' : '';
    g.setAttribute('aria-label', label + ' : ' + fmtPercent(p));
    g.textContent = fmtPercent(p);
    return g;
  }

  function renderStatus() {
    var s = data.status;
    var list = $('status-list');
    clear(list);
    if (!s) {
      return;
    }
    var about = data.about;
    tile(
      list,
      'Relais',
      'v' + s.relay_version,
      about ? 'en service depuis ' + fmtDuration(about.uptime_secs) : null
    );
    var used = s.quota.used_bytes;
    var max = s.quota.max_bytes;
    var p = percent(used, max);
    tile(
      list,
      'Stockage (tous les espaces)',
      fmtBytes(used),
      'sur ' + fmtBytes(max) + (p === null ? '' : ' (' + fmtPercent(p) + ')'),
      gauge(used, max, 'Stockage utilisé')
    );
    tile(list, 'Disque libre', fmtBytes(s.disk_free_bytes), 'sur le volume des données');
    var members = 0;
    var active = 0;
    data.spaces.forEach(function (sp) {
      members += Number(sp.members || 0);
      active += Number(sp.active || 0);
    });
    tile(
      list,
      'Espaces',
      fmtCount(s.spaces),
      plural(members, 'appareil', 'appareils') + ', ' + plural(active, 'actif', 'actifs') +
        (s.pending_spaces ? ' ; ' + plural(s.pending_spaces, 'en création', 'en création') : '')
    );
    var fcm = s.fcm || {};
    var fcmSub = null;
    if (fcm.last_success_at) {
      fcmSub = 'dernier envoi réussi ' + rel(fcm.last_success_at);
    } else if (fcm.state === 'disabled') {
      fcmSub = 'compte de service FCM non configuré';
    }
    tile(list, 'Notifications (FCM)', fcmLabel(fcm), fcmSub);
    tile(
      list,
      'Invitations en attente',
      fmtCount(s.invitations_pending),
      s.invitations_pending ? 'jetons pas encore utilisés' : 'aucune'
    );
  }

  // Attention points: what requires action, derived from the metadata already read only.
  function renderAlerts() {
    var items = [];
    var s = data.status;
    if (s) {
      var fcm = s.fcm || {};
      if (fcm.state === 'auth_error') {
        items.push([
          'error',
          'Notifications :',
          'le compte de service FCM est refusé ou illisible ; les appareils ne sont plus réveillés.'
        ]);
      } else if (fcm.state === 'degraded') {
        items.push([
          'warn',
          'Notifications :',
          'des envois échouent' +
            (fcm.last_error && fcm.last_error.at ? ' (dernière erreur ' + rel(fcm.last_error.at) + ')' : '') +
            '.'
        ]);
      }
      var p = percent(s.quota.used_bytes, s.quota.max_bytes);
      if (p !== null && p >= 90) {
        items.push([
          p >= 98 ? 'error' : 'warn',
          'Stockage du relais :',
          fmtPercent(p) + ' du total utilisé ; au-delà, les envois sont refusés.'
        ]);
      }
      var limits = data.about && data.about.limits;
      if (limits && s.disk_free_bytes < Number(limits.min_free_disk_bytes) * 2) {
        items.push([
          'warn',
          'Disque :',
          'il reste ' + fmtBytes(s.disk_free_bytes) + ' ; sous ' +
            fmtBytes(limits.min_free_disk_bytes) + ', le relais refuse les écritures.'
        ]);
      }
    }
    data.spaces.forEach(function (sp) {
      if (sp.rotation_required) {
        items.push([
          'warn',
          'Espace ' + sp.id + ' :',
          'changement de clé en attente (après un retrait d’appareil) : un appareil membre doit ' +
            'renouveler la clé de l’espace, depuis l’application.'
        ]);
      }
      var q = percent(sp.used_bytes, sp.max_bytes);
      if (q !== null && q >= 90) {
        items.push(['warn', 'Espace ' + sp.id + ' :', fmtPercent(q) + ' de son quota utilisé.']);
      }
    });
    var refused = 0;
    var lastRefused = null;
    data.journal.forEach(function (e) {
      if (e.kind === 'sign_in_refused' || e.kind === 'enrollment_refused') {
        refused += Number(e.count || 1);
        if (!lastRefused || e.at > lastRefused) {
          lastRefused = e.at;
        }
      }
    });
    if (refused) {
      items.push([
        'warn',
        'Connexions :',
        plural(refused, 'tentative refusée', 'tentatives refusées') + ' dans le journal, la ' +
          'dernière ' + rel(lastRefused) + '. Si ce n’était pas toi, change le chemin du panneau ' +
          '(tomi-relay admin web --rotate-path).'
      ]);
    }
    var list = $('alerts');
    clear(list);
    items.forEach(function (it) {
      var li = el('li', it[0] === 'error' ? 'error' : '');
      li.appendChild(el('strong', null, it[1]));
      li.appendChild(document.createTextNode(' ' + it[2]));
      list.appendChild(li);
    });
    list.hidden = items.length === 0;
    $('alerts-none').hidden = items.length !== 0 || !s;
  }

  function loadStatus() {
    return read('api/status').then(function (s) {
      data.status = s;
    });
  }

  // ----- spaces ----------------------------------------------------------------------------

  function loadSpaces() {
    return read('api/spaces').then(function (d) {
      data.spaces = d.spaces || [];
      renderSpaces();
    });
  }

  function usageCell(used, max, source) {
    var box = el('div');
    var g = gauge(used, max, 'Quota utilisé');
    if (g) {
      box.appendChild(g);
    }
    var p = percent(used, max);
    box.appendChild(
      el(
        'span',
        null,
        max === null || max === undefined
          ? fmtBytes(used) + ' · aucune limite'
          : fmtBytes(used) + ' sur ' + fmtBytes(max) + ' (' + fmtPercent(p) + ')'
      )
    );
    if (source === 'space') {
      box.appendChild(el('span', 'sub', 'quota propre à l’espace'));
    }
    return box;
  }

  function renderSpaces() {
    var body = $('spaces-body');
    clear(body);
    var spaces = sorted(data.spaces, sorts.spaces);
    $('spaces-empty').hidden = spaces.length !== 0;
    $('spaces-table').hidden = spaces.length === 0;
    spaces.forEach(function (s) {
      var row = document.createElement('tr');
      if (s.id === selectedSpace) {
        row.className = 'selected';
      }
      var id = el('span', 'copyable');
      id.appendChild(el('span', 'mono', s.id));
      id.appendChild(copyButton(s.id, 'Identifiant', 'Copier l’identifiant ' + s.id));
      cell(row, id, 'Identifiant', true);
      cell(row, timeEl(s.created_at), 'Créé');
      cell(row, timeEl(s.last_activity_at), 'Dernière activité');
      var devs = el('div');
      devs.appendChild(el('span', null, fmtCount(s.active) + ' actif' + (s.active > 1 ? 's' : '') +
        ' sur ' + fmtCount(s.members)));
      if (s.present !== undefined) {
        devs.appendChild(el('span', 'sub', plural(s.present, 'présent', 'présents') + ' en ce moment'));
      }
      cell(row, devs, 'Appareils');
      var content = el('div');
      content.appendChild(el('span', null, plural(s.items, 'élément', 'éléments')));
      content.appendChild(el('span', 'sub', plural(s.blobs, 'fichier chiffré', 'fichiers chiffrés') +
        ' · ' + plural(s.events, 'événement', 'événements')));
      cell(row, content, 'Contenu');
      cell(row, usageCell(s.used_bytes, s.max_bytes, s.quota_source), 'Utilisation');
      var state = el('div', 'badges');
      if (s.rotation_required) {
        state.appendChild(badge('Changement de clé en attente', 'warn'));
      }
      if (s.members > 0 && !s.active) {
        state.appendChild(badge('Aucun appareil actif', 'muted'));
      }
      if (!state.firstChild) {
        state.appendChild(badge('Normal', 'good'));
      }
      cell(row, state, 'État');
      var b = el('button', 'secondary', s.id === selectedSpace ? 'Appareils (ouverts)' : 'Appareils');
      b.type = 'button';
      b.setAttribute('aria-label', 'Voir les appareils de l’espace ' + s.id);
      b.setAttribute('aria-controls', 'devices-section');
      b.setAttribute('aria-expanded', s.id === selectedSpace ? 'true' : 'false');
      b.setAttribute('data-space', s.id);
      b.addEventListener('click', function () {
        selectSpace(s.id);
      });
      cell(row, b, null);
      body.appendChild(row);
    });
  }

  // ----- devices of a space --------------------------------------------------------------

  function selectSpace(id) {
    selectedSpace = id;
    renderSpaces();
    loadDevices(id, true);
  }

  function closeDevices() {
    var id = selectedSpace;
    selectedSpace = null;
    $('devices-section').hidden = true;
    renderSpaces();
    var back = id ? document.querySelector('button[data-space="' + CSS.escape(id) + '"]') : null;
    if (back) {
      back.focus();
    }
  }

  function spaceFacts(s) {
    var list = $('space-facts');
    clear(list);
    if (!s) {
      return;
    }
    function fact(label, value) {
      list.appendChild(el('dt', null, label));
      var dd = el('dd');
      if (value instanceof Node) {
        dd.appendChild(value);
      } else {
        dd.textContent = value;
      }
      list.appendChild(dd);
    }
    var created = el('span');
    created.appendChild(timeEl(s.created_at));
    created.appendChild(document.createTextNode(' (' + fmtDate(s.created_at) + ')'));
    fact('Créé', created);
    fact('Éléments vivants', fmtCount(s.items));
    fact('Fichiers chiffrés', fmtCount(s.blobs));
    var log = plural(s.events, 'événement retenu', 'événements retenus');
    if (s.head_seq !== null && s.head_seq !== undefined) {
      log += ' (n° ' + fmtCount(s.retention_floor_seq) + ' à ' + fmtCount(s.head_seq) + ')';
    }
    fact('Journal de l’espace', log);
    fact(
      'Changement de clé',
      s.rotation_required ? 'en attente : un appareil membre doit renouveler la clé, depuis l’application' : 'rien en attente'
    );
    fact('Quota', s.max_bytes === null || s.max_bytes === undefined
      ? 'aucune limite'
      : fmtBytes(s.max_bytes) + (s.quota_source === 'space' ? ' (propre à l’espace)' : ' (quota par défaut)'));
  }

  function spaceCommands(id) {
    var list = $('space-commands');
    clear(list);
    [
      ['tomi-relay admin status --space ' + id, 'l’état détaillé de l’espace'],
      ['tomi-relay admin devices --space ' + id, 'les appareils, avec leur identifiant complet'],
      ['tomi-relay admin quota --space ' + id + ' OCTETS', 'un quota propre à l’espace (OCTETS, ou none / default)'],
      ['tomi-relay admin revoke --space ' + id + ' ID_APPAREIL', 'retirer un appareil (ID_APPAREIL : voir la commande devices)'],
      ['tomi-relay admin reset --space ' + id, 'supprimer l’espace (demande une confirmation)']
    ].forEach(function (c) {
      var li = el('li');
      var line = el('span', 'copyable');
      line.appendChild(el('code', null, c[0]));
      line.appendChild(copyButton(c[0], 'Commande', 'Copier la commande : ' + c[1]));
      li.appendChild(line);
      li.appendChild(el('span', 'sub', c[1]));
      list.appendChild(li);
    });
  }

  var PLATFORMS = { android: 'Android', ios: 'iOS', windows: 'Windows', macos: 'macOS', linux: 'Linux', cli: 'CLI' };

  function renderDevices() {
    var body = $('devices-body');
    clear(body);
    var devices = sorted(data.devices, sorts.devices);
    $('devices-empty').hidden = devices.length !== 0;
    $('devices-table').hidden = devices.length === 0;
    devices.forEach(function (d) {
      var row = document.createElement('tr');
      cell(row, d.name, 'Nom', true);
      cell(row, PLATFORMS[d.platform] || d.platform, 'Plateforme');
      cell(row, d.app_version || '—', 'Version');
      cell(row, timeEl(d.registered_at), 'Ajouté');
      cell(row, timeEl(d.last_seen_at), 'Dernière activité');
      var state = el('div', 'badges');
      state.appendChild(d.active ? badge('Actif', 'good') : badge('Inactif', 'muted'));
      if (d.present) {
        state.appendChild(badge('Présent', 'good'));
      }
      state.appendChild(d.push ? badge('Notifications', '') : badge('Sans notifications', 'muted'));
      cell(row, state, 'État');
      if (d.fingerprint) {
        var fp = el('span', 'copyable');
        fp.appendChild(el('span', 'mono', d.fingerprint));
        fp.appendChild(copyButton(d.fingerprint, 'Empreinte', 'Copier l’empreinte de ' + d.name));
        cell(row, fp, 'Empreinte');
      } else {
        cell(row, '—', 'Empreinte');
      }
      body.appendChild(row);
    });
  }

  function loadDevices(spaceId, focus) {
    $('devices-space').textContent = spaceId;
    $('devices-section').hidden = false;
    spaceFacts(data.spaces.filter(function (s) {
      return s.id === spaceId;
    })[0]);
    spaceCommands(spaceId);
    $('devices-error').hidden = true;
    if (focus) {
      $('h-devices').focus();
    }
    return read('api/devices?space=' + encodeURIComponent(spaceId))
      .then(function (d) {
        if (selectedSpace !== spaceId) {
          return; // another space was chosen in the meantime
        }
        data.devices = d.devices || [];
        renderDevices();
      })
      .catch(function (err) {
        if (err && err.message === 'session') {
          throw err;
        }
        if (selectedSpace !== spaceId) {
          return; // the error of a space we left is not shown under another
        }
        data.devices = [];
        renderDevices();
        $('devices-empty').hidden = true;
        $('devices-error').textContent = 'Impossible de lire les appareils de cet espace. Réessaie avec « Actualiser ».';
        $('devices-error').hidden = false;
      })
      .catch(function (err) {
        failure(err, 'Impossible de lire les appareils.');
      });
  }

  // ----- default quota ----------------------------------------------------------------

  var QUOTA_SOURCES = {
    panel: 'réglé depuis ce panneau',
    config: 'configuration du relais (max_space_bytes)',
    none: 'rien n’est réglé : aucune limite par espace'
  };

  function renderQuota(q) {
    data.quota = q;
    var list = $('quota-facts');
    clear(list);
    list.appendChild(el('dt', null, 'Quota par défaut'));
    list.appendChild(el('dd', null, q.bytes === null ? 'aucune limite par espace' : fmtBytes(q.bytes)));
    list.appendChild(el('dt', null, 'Origine'));
    list.appendChild(el('dd', null, QUOTA_SOURCES[q.source] || q.source));
    list.appendChild(el('dt', null, 'Total du relais (tous espaces)'));
    list.appendChild(el('dd', null, fmtBytes(q.relay_max_bytes)));
    var input = $('quota-mib');
    input.min = String(Math.ceil(q.min_bytes / 1048576));
    input.setAttribute('data-min', String(q.min_bytes));
    input.setAttribute('data-max', String(q.max_bytes));
    input.value = q.bytes === null ? '' : String(Math.round(q.bytes / 1048576));
    previewQuota();
  }

  function loadQuota() {
    return read('api/quota').then(function (d) {
      if (!d.default) {
        throw new Error('failed');
      }
      renderQuota(d.default);
    });
  }

  // The number of bytes the input is worth, or null if it is not acceptable.
  function quotaBytes() {
    var text = $('quota-mib').value.trim();
    var min = Number($('quota-mib').getAttribute('data-min') || 1048576);
    var max = Number($('quota-mib').getAttribute('data-max') || 9007199254740991);
    if (!/^[0-9]{1,9}$/.test(text)) {
      return null;
    }
    var bytes = Number(text) * 1048576;
    return bytes < min || bytes > max ? null : bytes;
  }

  function previewQuota() {
    var text = $('quota-mib').value.trim();
    var out = $('quota-preview');
    if (!text) {
      out.textContent = '';
      return;
    }
    var bytes = quotaBytes();
    out.textContent = bytes === null ? 'Un nombre entier de Mio, au moins 1.' : '= ' + fmtBytes(bytes) + ' par espace';
  }

  // `value`: a number of bytes, 0 (no per-space limit) or null (back to the relay configuration
  // of the relay: `max_space_bytes`, otherwise no limit).
  function saveQuota(value, done) {
    var buttons = ['quota-save', 'quota-unlimited', 'quota-reset'];
    buttons.forEach(function (id) {
      $(id).disabled = true;
    });
    return api('POST', 'api/quota', { default_bytes: value })
      .then(function (r) {
        if (r.status === 401) {
          throw new Error('session');
        }
        if (r.status === 400) {
          say('Valeur refusée : au moins 1 Mio.', 'error');
          return null;
        }
        if (r.status !== 200 || !r.data || !r.data.default) {
          throw new Error('failed');
        }
        renderQuota(r.data.default);
        say(done, 'ok');
        return refreshAll();
      })
      .catch(function (err) {
        failure(err, 'Impossible de modifier le quota.');
      })
      .then(function () {
        buttons.forEach(function (id) {
          $(id).disabled = false;
        });
      });
  }

  function submitQuota(event) {
    event.preventDefault();
    var bytes = quotaBytes();
    if (bytes === null) {
      say('Saisis un nombre entier de Mio (au moins 1).', 'error');
      $('quota-mib').focus();
      return;
    }
    saveQuota(bytes, 'Quota par défaut enregistré : ' + fmtBytes(bytes) + ' par espace.');
  }

  function quotaUnlimited() {
    confirmAction(
      'Retirer le plafond par espace ?',
      'Seul le total du relais (' + fmtBytes(data.quota ? data.quota.relay_max_bytes : null) +
        ') limitera alors chaque espace : un espace pourrait le remplir pour les autres. Les ' +
        'espaces qui ont un quota propre le gardent.',
      'Retirer le plafond'
    ).then(function (yes) {
      if (yes) {
        saveQuota(0, 'Plus de plafond par espace (le total du relais s’applique).');
      }
    });
  }

  // ----- invitations ----------------------------------------------------------------------

  function renderInvitations() {
    var list = $('invitations-list');
    clear(list);
    var items = data.invitations;
    $('invitations-empty').hidden = items.length !== 0;
    items.forEach(function (i) {
      var li = el('li');
      li.appendChild(document.createTextNode('Invitation '));
      li.appendChild(el('span', 'mono', i.id));
      li.appendChild(document.createTextNode(', créée '));
      li.appendChild(timeEl(i.created_at));
      list.appendChild(li);
    });
  }

  function loadInvitations() {
    return read('api/invitations').then(function (d) {
      data.invitations = d.invitations || [];
      renderInvitations();
    });
  }

  // Draws the QR of the creation link in a <canvas> created here (never present in the page before). The QR
  // encodes exactly the string received (the creation link `<host>/create#<token>`). Black on white, quiet zone of 4 modules, whatever the
  // theme: an inverted-colour QR cannot be scanned. No network call, no export.
  function showQr(text) {
    clearQr();
    try {
      var matrix = qrMatrix(text);
      var modules = matrix.length + 8;
      var scale = Math.max(4, Math.round(220 / modules));
      var canvas = document.createElement('canvas');
      canvas.width = modules * scale;
      canvas.height = modules * scale;
      canvas.setAttribute('role', 'img');
      canvas.setAttribute('aria-label', 'QR code du lien de création d’espace');
      var ctx = canvas.getContext('2d');
      if (!ctx) {
        throw new Error('canvas');
      }
      ctx.fillStyle = '#ffffff';
      ctx.fillRect(0, 0, canvas.width, canvas.height);
      ctx.fillStyle = '#000000';
      for (var y = 0; y < matrix.length; y++) {
        for (var x = 0; x < matrix.length; x++) {
          if (matrix[y][x]) {
            ctx.fillRect((x + 4) * scale, (y + 4) * scale, scale, scale);
          }
        }
      }
      $('token-qr').appendChild(canvas);
      $('token-qr-wrap').hidden = false;
    } catch (e) {
      // The token text stays there: the QR is only a complement.
      clearQr();
    }
  }

  function clearQr() {
    var holder = $('token-qr');
    var canvases = holder.getElementsByTagName('canvas');
    for (var i = 0; i < canvases.length; i++) {
      canvases[i].width = 0; // clears the drawing
      canvases[i].height = 0;
    }
    clear(holder);
    $('token-qr-wrap').hidden = true;
  }

  // The token does not stay displayed indefinitely: 10 minutes at most, and never after leaving
  // the page. It is written nowhere else (neither local storage nor log).
  var TOKEN_SHOWN_MS = 10 * 60000;
  var tokenTimer = null;

  function hideToken() {
    if (tokenTimer) {
      clearTimeout(tokenTimer);
      tokenTimer = null;
    }
    $('token').textContent = '';
    $('token-link').textContent = '';
    $('token-link-wrap').hidden = true;
    $('token-link-copy').hidden = true;
    clearQr();
    $('token-box').hidden = true;
  }

  function invite(mode) {
    var pending = data.invitations.length;
    var ask = mode === 'rotate' && pending > 0
      ? confirmAction(
        'Remplacer les invitations en attente ?',
        (pending > 1
          ? 'Les ' + pending + ' invitations en attente cesseront de fonctionner'
          : 'L’invitation en attente cessera de fonctionner') +
          ' : un jeton déjà donné ne permettra plus de créer d’espace. Une nouvelle invitation ' +
          'sera créée, et son jeton affiché une seule fois. Les espaces existants ne changent pas.',
        'Remplacer'
      )
      : Promise.resolve(true);
    ask.then(function (yes) {
      if (!yes) {
        return null;
      }
      var buttons = [$('invite-new'), $('invite-rotate')];
      buttons.forEach(function (b) {
        b.disabled = true;
      });
      return api('POST', 'api/invitations', { mode: mode })
        .then(function (r) {
          if (r.status === 401) {
            throw new Error('session');
          }
          if (r.status !== 200 || !r.data || !r.data.token) {
            throw new Error('failed');
          }
          $('token').textContent = r.data.token;
          // The link comes from the server (public_url): never built from the browser address.
          var link = typeof r.data.link === 'string' ? r.data.link : '';
          $('token-link').textContent = link;
          $('token-link-wrap').hidden = !link;
          $('token-link-copy').hidden = !link;
          showQr(link || r.data.token);
          $('token-box').hidden = false;
          $('token-title').focus();
          if (tokenTimer) {
            clearTimeout(tokenTimer);
          }
          tokenTimer = setTimeout(function () {
            hideToken();
            say('Le jeton a été masqué (10 minutes écoulées).', 'ok');
          }, TOKEN_SHOWN_MS);
          say('Invitation créée. Copie le jeton maintenant : il ne sera plus affiché.', 'ok');
          return refreshAll();
        })
        .catch(function (err) {
          failure(err, 'Impossible de créer l’invitation.');
        })
        .then(function () {
          buttons.forEach(function (b) {
            b.disabled = false;
          });
        });
    });
  }

  function copyToken() {
    copyText($('token').textContent, 'Jeton');
  }

  function copyLink() {
    copyText($('token-link').textContent, 'Lien');
  }

  // ----- administration log ---------------------------------------------------------

  var JOURNAL_LABELS = {
    signed_in: 'Connexion avec la clé d’accès',
    sign_in_refused: 'Connexion refusée',
    enrollment_refused: 'Code d’enrôlement refusé',
    signed_out: 'Déconnexion',
    passkey_registered: 'Clé d’accès créée',
    invitation_created: 'Invitation créée',
    invitations_replaced: 'Invitations en attente remplacées',
    default_quota_set: 'Quota par défaut réglé',
    default_quota_unlimited: 'Quota par défaut : sans limite par espace',
    default_quota_config: 'Quota par défaut : retour à la configuration du relais',
    path_rotated: 'Chemin du panneau changé sur le serveur (sessions fermées)',
    passkey_reset: 'Clé d’accès supprimée sur le serveur'
  };

  function renderJournal() {
    var list = $('journal-list');
    clear(list);
    var entries = data.journal;
    $('journal-empty').hidden = entries.length !== 0;
    entries.forEach(function (e) {
      var warn = e.kind === 'sign_in_refused' || e.kind === 'enrollment_refused';
      var li = el('li', warn ? 'warn' : '');
      li.appendChild(timeEl(e.at));
      var what = JOURNAL_LABELS[e.kind] || e.kind;
      if (e.kind === 'default_quota_set' && e.bytes !== undefined) {
        what += ' : ' + fmtBytes(e.bytes) + ' par espace';
      }
      if (e.count > 1) {
        what += ' (' + e.count + ' fois, depuis ' + rel(e.first_at) + ')';
      }
      li.appendChild(el('span', 'what', what));
      list.appendChild(li);
    });
  }

  function loadJournal() {
    return read('api/journal').then(function (d) {
      data.journal = d.entries || [];
      renderJournal();
    });
  }

  // ----- about and diagnostics -------------------------------------------------------------

  function facts(listId, rows) {
    var list = $(listId);
    clear(list);
    rows.forEach(function (r) {
      list.appendChild(el('dt', null, r[0]));
      list.appendChild(el('dd', null, r[1]));
    });
  }

  function limitRows(l) {
    return [
      ['Total du relais', fmtBytes(l.max_relay_bytes)],
      ['Quota par espace (configuration)', l.max_space_bytes ? fmtBytes(l.max_space_bytes) : 'aucun'],
      ['Appareils par espace', fmtCount(l.max_devices)],
      ['Taille d’un élément (chiffré)', fmtBytes(l.max_envelope_bytes)],
      ['Taille d’un fichier', fmtBytes(l.max_blob_bytes)],
      ['Fichiers d’un élément, au total', fmtBytes(l.max_item_blob_bytes)],
      ['Durée de vie la plus longue d’un élément à échéance', fmtDuration(l.max_ttl_secs)],
      ['Rétention du journal d’un espace', l.event_retention_days + ' j'],
      ['Appareil compté inactif après', l.inactive_after_days + ' j sans connexion'],
      ['Purge des éléments échus', 'toutes les ' + fmtDuration(l.purge_interval_secs)],
      ['Fichier jamais rattaché supprimé après', fmtDuration(l.blob_orphan_ttl_secs)],
      ['Session d’un appareil', fmtDuration(l.session_ttl_secs)],
      ['Espace disque gardé libre', fmtBytes(l.min_free_disk_bytes)]
    ];
  }

  function renderAbout() {
    var a = data.about;
    if (!a) {
      return;
    }
    var skew = Math.round(serverOffset / 1000);
    facts('about-facts', [
      ['Version du relais', a.relay_version],
      ['En service depuis', fmtDuration(a.uptime_secs) + ' (démarré le ' + fmtDate(a.started_at) + ')'],
      ['Horloge du serveur', fmtDate(a.server_time) +
        (Math.abs(skew) >= 60 ? ' (écart de ' + fmtDuration(Math.abs(skew)) + ' avec cet appareil)' : '')],
      ['Notifications push', a.push_configured ? 'configurées' : 'non configurées'],
      ['Ta session', 'se ferme après ' + fmtDuration(a.session.idle_ttl_secs) + ' sans activité, au plus tard ' +
        rel(a.session.absolute_ends_at)]
    ]);
    facts('limits-facts', limitRows(a.limits || {}));
  }

  function loadAbout() {
    var asked = Date.now();
    return read('api/about').then(function (a) {
      var at = Date.parse(a.server_time);
      if (!isNaN(at)) {
        // Midpoint of the round trip: precise enough for "3 min ago".
        serverOffset = at - Math.round((asked + Date.now()) / 2);
      }
      data.about = a;
      setSession(a);
      renderAbout();
    });
  }

  // A copyable summary for a bug report: versions, sizes, counts and
  // states. No space identifier, device name, fingerprint, token or panel URL.
  function diagnosticText() {
    var lines = ['Tomi : diagnostic du relais', 'Établi le ' + new Date().toISOString()];
    var s = data.status;
    var a = data.about;
    if (a) {
      lines.push('Version du relais : ' + a.relay_version);
      lines.push('En service depuis : ' + fmtDuration(a.uptime_secs) + ' (démarré ' + a.started_at + ')');
      lines.push('Écart d’horloge serveur / navigateur : ' + Math.round(serverOffset / 1000) + ' s');
    }
    if (s) {
      lines.push('Espaces : ' + s.spaces + (s.pending_spaces ? ' (+ ' + s.pending_spaces + ' en création)' : ''));
      var members = 0;
      var active = 0;
      var present = 0;
      var rotations = 0;
      data.spaces.forEach(function (sp) {
        members += Number(sp.members || 0);
        active += Number(sp.active || 0);
        present += Number(sp.present || 0);
        rotations += sp.rotation_required ? 1 : 0;
      });
      lines.push('Appareils (tous espaces) : ' + members + ', dont ' + active + ' actifs et ' + present + ' présents');
      lines.push('Changements de clé en attente : ' + rotations + ' espace(s)');
      var p = percent(s.quota.used_bytes, s.quota.max_bytes);
      lines.push('Stockage : ' + fmtBytes(s.quota.used_bytes) + ' sur ' + fmtBytes(s.quota.max_bytes) +
        (p === null ? '' : ' (' + fmtPercent(p) + ')'));
      if (s.default_space_quota) {
        lines.push('Quota par défaut d’un espace : ' +
          (s.default_space_quota.bytes === null ? 'aucune limite' : fmtBytes(s.default_space_quota.bytes)) +
          ' (origine : ' + s.default_space_quota.source + ')');
      }
      lines.push('Disque libre : ' + fmtBytes(s.disk_free_bytes));
      var fcm = s.fcm || {};
      var f = 'Notifications (FCM) : ' + (fcm.state || 'unknown');
      if (fcm.last_success_at) {
        f += ', dernier succès ' + fcm.last_success_at;
      }
      if (fcm.last_error) {
        f += ', dernière erreur ' + fcm.last_error.class + (fcm.last_error.at ? ' le ' + fcm.last_error.at : '');
      }
      if (fcm.unregistered_tokens_7d !== undefined) {
        f += ', jetons désinscrits sur 7 j : ' + fcm.unregistered_tokens_7d;
      }
      lines.push(f);
      lines.push('Invitations en attente : ' + s.invitations_pending);
    }
    if (a && a.limits) {
      var l = a.limits;
      lines.push('Limites : total ' + l.max_relay_bytes + ' o ; quota configuré ' + (l.max_space_bytes || 'aucun') +
        ' ; appareils ' + l.max_devices + ' ; élément ' + l.max_envelope_bytes + ' o ; fichier ' + l.max_blob_bytes +
        ' o ; rétention ' + l.event_retention_days + ' j ; inactivité ' + l.inactive_after_days + ' j');
    }
    var refused = 0;
    data.journal.forEach(function (e) {
      if (e.kind === 'sign_in_refused' || e.kind === 'enrollment_refused') {
        refused += Number(e.count || 1);
      }
    });
    lines.push('Connexions refusées au panneau (dans le journal, 100 lignes au plus) : ' + refused);
    lines.push('Navigateur : ' + navigator.userAgent);
    return lines.join('\n');
  }

  function renderDiagnostic() {
    $('diagnostic').textContent = diagnosticText();
  }

  // ----- refresh ----------------------------------------------------------------------

  function failure(err, text) {
    if (err && err.message === 'session') {
      lostSession();
    } else {
      say(text, 'error');
    }
  }

  // A source that fails does not stop the others from showing: its error is written in its
  // section. An ended session, on the other hand, returns to sign-in.
  function guarded(promise, errId, text) {
    var box = $(errId);
    return promise.then(
      function () {
        box.hidden = true;
        box.textContent = '';
      },
      function (err) {
        if (err && err.message === 'session') {
          throw err;
        }
        box.textContent = text;
        box.hidden = false;
      }
    );
  }

  var refreshing = null;
  var refreshAgain = false;

  // Re-reads everything, without losing the open space or the position on the page. A request made
  // during a refresh (after an action, for example) starts a second one at the end:
  // the first one may have read the data from before the action.
  function refreshAll() {
    if (refreshing) {
      refreshAgain = true;
      return refreshing;
    }
    var button = $('refresh');
    button.disabled = true;
    button.textContent = 'Actualisation…';
    $('view-dashboard').setAttribute('aria-busy', 'true');
    var again = 'Réessaie avec « Actualiser ».';
    // First the server time (the "ago" counts depend on it), then the rest in parallel.
    refreshing = guarded(loadAbout(), 'about-error', 'Impossible de lire les informations du relais. ' + again)
      .then(function () {
        return Promise.all([
          guarded(loadStatus(), 'status-error', 'Impossible de lire l’état du relais. ' + again),
          guarded(loadSpaces(), 'spaces-error', 'Impossible de lire les espaces. ' + again),
          guarded(loadInvitations(), 'invitations-error', 'Impossible de lire les invitations. ' + again),
          guarded(loadQuota(), 'quota-error', 'Impossible de lire le quota par défaut. ' + again),
          guarded(loadJournal(), 'journal-error', 'Impossible de lire le journal. ' + again)
        ]);
      })
      .then(function () {
        if (selectedSpace) {
          var still = data.spaces.some(function (s) {
            return s.id === selectedSpace;
          });
          if (still) {
            return loadDevices(selectedSpace, false);
          }
          selectedSpace = null;
          $('devices-section').hidden = true;
          renderSpaces();
        }
        return null;
      })
      .then(function () {
        renderStatus();
        renderAlerts();
        renderDiagnostic();
        var now = new Date();
        $('updated-at').textContent = 'Mis à jour à ' + now.toLocaleTimeString('fr-FR', {
          hour: '2-digit',
          minute: '2-digit'
        });
      })
      .catch(function (err) {
        failure(err, 'Impossible de lire l’état du relais.');
      })
      .then(function () {
        button.disabled = false;
        button.textContent = 'Actualiser';
        $('view-dashboard').removeAttribute('aria-busy');
        refreshing = null;
        if (refreshAgain) {
          refreshAgain = false;
          if (session.idleEnd) {
            return refreshAll(); // session still open: re-read
          }
        }
        return null;
      });
    return refreshing;
  }

  function openDashboard() {
    hideToken();
    selectedSpace = null;
    $('devices-section').hidden = true;
    show('loading');
    return refreshAll().then(function () {
      // A session lost during the read has already led back to sign-in.
      if ($('view-login').hidden) {
        show('dashboard');
      }
    });
  }

  // ----- startup -----------------------------------------------------------------------

  function start() {
    loadTheme();
    $('theme').addEventListener('change', saveTheme);
    $('toast-close').addEventListener('click', function () {
      say('');
    });
    $('enroll-form').addEventListener('submit', enroll);
    $('login-submit').addEventListener('click', login);
    $('logout').addEventListener('click', logout);
    $('refresh').addEventListener('click', function () {
      refreshAll();
    });
    $('devices-close').addEventListener('click', closeDevices);
    $('invite-new').addEventListener('click', function () {
      invite('new');
    });
    $('invite-rotate').addEventListener('click', function () {
      invite('rotate');
    });
    $('quota-form').addEventListener('submit', submitQuota);
    $('quota-mib').addEventListener('input', previewQuota);
    $('quota-unlimited').addEventListener('click', quotaUnlimited);
    $('quota-reset').addEventListener('click', function () {
      saveQuota(null, 'Quota par défaut rétabli à celui de la configuration du relais.');
    });
    $('token-copy').addEventListener('click', copyToken);
    $('token-link-copy').addEventListener('click', copyLink);
    $('token-hide').addEventListener('click', hideToken);
    $('diagnostic-copy').addEventListener('click', function () {
      renderDiagnostic();
      copyText($('diagnostic').textContent, 'Diagnostic');
    });
    // Leaving the page clears the displayed token (and its QR).
    window.addEventListener('pagehide', hideToken);
    setupSort('spaces-table', 'spaces', renderSpaces);
    setupSort('devices-table', 'devices', renderDevices);

    if (!webauthnAvailable()) {
      show('nojs');
      return;
    }
    api('GET', 'api/state')
      .then(function (r) {
        if (r.status !== 200 || !r.data) {
          throw new Error('failed');
        }
        if (r.data.authenticated) {
          return openDashboard();
        }
        show(r.data.enrolled ? 'login' : 'enroll');
        return null;
      })
      .catch(function () {
        say('Le relais ne répond pas. Recharge la page.', 'error');
        show('login');
      });
  }

  // Outside the browser (checking the QR encoder with Node): only the encoder is exposed and
  // the page is left alone. In the browser, `module` does not exist: this block does nothing.
  if (typeof module === 'object' && module !== null && module.exports) {
    module.exports = { qrMatrix: qrMatrix, qrEncode: qrEncode, qrDataCodewords: qrDataCodewords };
    return;
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', start);
  } else {
    start();
  }
})();
