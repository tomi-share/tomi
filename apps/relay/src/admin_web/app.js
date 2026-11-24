// Administration page of the Tomi relay.
// No dependency, no framework, no external resource. All text coming from the server
// is inserted with textContent, never interpreted as markup.
'use strict';

(function () {
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


  // ----- SVG icons (the sprite is in index.html) ----------------------------------------------

  // The SVG namespace is an identifier, not a resource: written in two parts so that no address
  // appears in the file.
  var SVG_NS = 'http:' + '//www.w3.org/2000/svg';

  function ico(name, size) {
    var svg = document.createElementNS(SVG_NS, 'svg');
    svg.setAttribute('class', 'ico' + (size ? ' ' + size : ''));
    svg.setAttribute('aria-hidden', 'true');
    var use = document.createElementNS(SVG_NS, 'use');
    use.setAttribute('href', '#i-' + name);
    svg.appendChild(use);
    return svg;
  }

  function setIcon(svg, name) {
    var use = svg.querySelector('use');
    if (use) {
      use.setAttribute('href', '#i-' + name);
    }
  }

  function textOf(id, text) {
    $(id).textContent = text;
  }

  function copyBtn(what, label) {
    var b = el('button', 'icon-btn sm');
    b.type = 'button';
    b.setAttribute('data-copy', what);
    b.setAttribute('aria-label', label);
    b.appendChild(ico('copy'));
    return b;
  }

  // ----- feedback: a success clears itself, an error stays until it is closed --------------

  var toastTimer = null;

  function say(text, kind) {
    var toast = $('toast');
    clearTimeout(toastTimer);
    if (!text) {
      toast.hidden = true;
      $('message').textContent = '';
      return;
    }
    var error = kind === 'error';
    toast.className = 'toast' + (error ? ' error' : '');
    setIcon($('toast-ico'), error ? 'error' : 'check');
    $('message').setAttribute('role', error ? 'alert' : 'status');
    $('toast-close').hidden = !error;
    toast.hidden = false;
    $('message').textContent = text;
    if (!error) {
      toastTimer = setTimeout(function () {
        toast.hidden = true;
      }, 6000);
    }
  }

  // ----- the screens without navigation, and the application -----------------------------------

  var AUTH = ['loading', 'nojs', 'enroll', 'login'];
  var FOCUS = { loading: 'h-loading', nojs: 'h-unsupported', enroll: 'h-enroll', login: 'h-login' };
  var VIEWS = {
    apercu: 'Vue d’ensemble',
    espaces: 'Espaces',
    invitations: 'Invitations',
    journal: 'Journal',
    reglages: 'Réglages'
  };

  var appShown = false;

  function showAuth(name) {
    appShown = false;
    document.querySelector('.side').hidden = true;
    document.querySelector('.appbar').hidden = true;
    Object.keys(VIEWS).forEach(function (v) {
      $('view-' + v).hidden = true;
    });
    document.body.classList.remove('has-detail');
    AUTH.forEach(function (v) {
      $('view-' + v).hidden = v !== name;
    });
    var target = $(FOCUS[name]);
    if (target) {
      target.focus();
    }
  }

  function showApp() {
    appShown = true;
    AUTH.forEach(function (v) {
      $('view-' + v).hidden = true;
    });
    document.querySelector('.side').hidden = false;
    document.querySelector('.appbar').hidden = false;
    route();
  }

  // ----- the data of the last read: what is needed to redraw without asking again -------------

  var data = {
    status: null,
    spaces: [],
    about: null,
    invitations: [],
    journal: null,
    keys: null,
    trash: null,
    quota: null,
    devices: []
  };
  var sorts = { spaces: { key: 'activity', dir: -1 }, devices: { key: 'activity', dir: -1 } };
  var selectedSpace = null;

  function spaceById(id) {
    for (var i = 0; i < data.spaces.length; i++) {
      if (data.spaces[i].id === id) {
        return data.spaces[i];
      }
    }
    return null;
  }

  // ----- theme (a local preference of this browser: nothing secret) ---------------------------

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
    var radios = document.getElementsByName('theme');
    for (var i = 0; i < radios.length; i++) {
      radios[i].checked = radios[i].value === value;
    }
    applyTheme(value);
  }

  function saveTheme(event) {
    var value = event.target.value;
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

  // ----- clipboard -------------------------------------------------------------------------------

  function copyText(text, what) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(text).then(
        function () {
          say(what + ' copié.');
        },
        function () {
          say('Copie impossible : sélectionne le texte et copie-le à la main.', 'error');
        }
      );
    }
    say('Copie impossible : sélectionne le texte et copie-le à la main.', 'error');
    return Promise.resolve();
  }

  // The text a copy button copies: its command, the id next to it, or the one of the open space.
  function copyValue(button) {
    var cmd = button.closest('.cmd');
    if (cmd) {
      return cmd.querySelector('code').textContent;
    }
    if (button.id === 'd-copy') {
      return selectedSpace || '';
    }
    var row = button.closest('th, dd');
    if (row) {
      var node = row.querySelector('.space-link, .mono');
      return node ? node.textContent : '';
    }
    return '';
  }

  // ----- server calls ----------------------------------------------------------------------------

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
      // The server pushes the end of the session back on every authenticated request, even one
      // refused afterwards: only a 401 proves that it did not.
      if (response.status !== 401) {
        noteActivity();
      }
      if (response.status === 204) {
        return { status: 204, data: null };
      }
      return response.text().then(function (text) {
        var parsed = null;
        try {
          parsed = text ? JSON.parse(text) : null;
        } catch (e) {
          parsed = null;
        }
        return { status: response.status, data: parsed };
      });
    });
  }

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

  // ----- session ---------------------------------------------------------------------------------

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

  // The session is written in the sheet (⋮, or the foot of the sidebar); only a session that ends in
  // 5 minutes or less comes up into the bar, as a chip.
  function tickSession() {
    var chip = $('session-chip');
    if (!session.idleEnd || !session.absoluteEnd) {
      textOf('session-line', '');
      textOf('session-note', '');
      chip.hidden = true;
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
    textOf('session-line', 'Session : ' + (minutes <= 5 ? 'se ferme dans ' : 'encore ') + fmtDuration(minutes * 60));
    textOf(
      'session-note',
      hard
        ? 'Limite de 8 h après la connexion : il faudra te reconnecter.'
        : 'Se ferme après 30 min sans activité, au plus tard dans ' + fmtDuration((session.absoluteEnd - Date.now()) / 1000) + '.'
    );
    chip.hidden = minutes > 5;
    chip.querySelector('span').textContent = minutes + ' min';
    document.body.classList.toggle('session-soon', minutes <= 5);
  }

  function resetScreens() {
    hideToken();
    stopSession();
    selectedSpace = null;
    closeSheet();
    if ($('stepup').open) {
      $('stepup').close();
    }
    if ($('confirm-dialog').open) {
      $('confirm-dialog').close();
    }
  }

  function lostSession(message, kind) {
    resetScreens();
    if (message === undefined) {
      say('La session a pris fin. Connecte-toi de nouveau.', 'error');
    } else if (message) {
      say(message, kind);
    }
    showAuth('login');
  }

  // ----- passkeys --------------------------------------------------------------------------------

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
        say('Clé d’accès créée. Connecte-toi maintenant.');
        showAuth('login');
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
        $('login-note').hidden = true;
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
    resetScreens();
    api('POST', 'api/logout', {}).then(
      function () {
        say('Tu es déconnecté.');
        showAuth('login');
      },
      function () {
        say('Déconnexion impossible : le serveur ne répond pas. La session se fermera d’elle-même.', 'error');
        showAuth('login');
      }
    );
  }

  // "Close every session": a protection, so the button is never red.
  function closeAllSessions() {
    confirmAction({
      title: 'Fermer toutes les sessions ?',
      text: 'Toutes les sessions du panneau seront fermées, celle-ci comprise. Ta clé d’accès reste valable : tu pourras te reconnecter tout de suite.',
      ok: 'Fermer toutes les sessions',
      tone: ''
    }).then(function (yes) {
      if (!yes) {
        return;
      }
      api('POST', 'api/sessions/close-all', {}).then(
        function (r) {
          if (r.status !== 204 && r.status !== 401) {
            say('Impossible de fermer les sessions. Réessaie.', 'error');
            return;
          }
          lostSession('Toutes les sessions du panneau sont fermées, celle-ci comprise. Reconnecte-toi avec ta clé d’accès.');
        },
        function () {
          say('Le relais ne répond pas. Actualise pour voir l’état réel.', 'error');
        }
      );
    });
  }

  // ----- the simple confirmation (session only): "Annuler" has the focus -----------------------

  var confirmFocus = null;

  function confirmAction(o) {
    var dlg = $('confirm-dialog');
    if (typeof dlg.showModal !== 'function') {
      return Promise.resolve(window.confirm(o.title + '\n\n' + o.text));
    }
    textOf('confirm-title', o.title);
    textOf('confirm-text', o.text);
    textOf('confirm-ok', o.ok);
    $('confirm-ok').className = 'btn' + (o.tone === undefined ? ' danger' : o.tone ? ' ' + o.tone : '');
    var list = $('confirm-list');
    clear(list);
    list.hidden = !(o.list && o.list.length);
    (o.list || []).forEach(function (row) {
      var li = el('li');
      li.appendChild(el('span', 'mono', row[0]));
      li.appendChild(el('span', null, row[1]));
      list.appendChild(li);
    });
    confirmFocus = document.activeElement;
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
        if (confirmFocus && document.contains(confirmFocus)) {
          confirmFocus.focus();
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
      dlg.addEventListener('cancel', onCancel);
      dlg.showModal();
      cancel.focus();
    });
  }

  // ===== step-up: a passkey tied to the action ===================================================
  //
  // The sheet shows the summary the SERVER built (`api/stepup/options`): the authenticator shows
  // only the name of the site, never the action. `data-state` of #stepup: review, pending,
  // cancelled, refused, error, expired, limit, throttled, changed, lastdevice, devicekey, closed
  // (see DECISION-ACTIONS.md).

  var su = $('stepup');
  var stepCfg = null;
  var stepTimer = null;
  var stepAbort = null;
  var stepFocus = null;
  var refusals = 0;

  var STEP_MSG = {
    cancelled: ['warn', 'warning', 'Annulé sur ton appareil', 'Rien n’a été fait. Tu peux réessayer quand tu veux.'],
    refused: ['', 'error', 'Clé d’accès refusée par le relais',
      'Cette clé n’est pas enregistrée sur ce relais, ou la vérification (empreinte, visage, code) n’a pas eu lieu. Rien n’a été fait.'],
    expired: ['warn', 'schedule', 'Délai de 2 min dépassé',
      'Ce défi n’est plus valable et rien n’a été fait. Recommence pour en obtenir un nouveau.'],
    error: ['', 'error', 'Le relais n’a pas répondu',
      'L’action a pu être faite, ou non. Actualise pour voir l’état réel avant de recommencer.'],
    closed: ['', 'lock', 'Session fermée par prudence',
      '5 refus de clé d’accès dans cette session. Ta clé d’accès reste valable : reconnecte-toi, puis regarde le journal si ce n’était pas toi.'],
    changed: ['warn', 'info', 'Déjà fait ailleurs',
      'La cible a changé depuis que tu as ouvert cette fenêtre. Rien n’a été fait : actualise pour voir l’état réel.'],
    lastdevice: ['warn', 'info', 'Dernier appareil de l’espace',
      'Un espace garde au moins un appareil. Rien n’a été fait. Pour en finir avec cet espace, supprime-le : une clé d’accès te sera demandée.'],
    devicekey: ['warn', 'lock', 'Clé liée à l’appareil exigée',
      'La suppression d’un espace demande une clé liée à l’appareil, et aucune n’est enregistrée. Rien n’a été fait. Ajoutes-en une sur le serveur avec « tomi-relay admin web enroll ».'],
    throttled: ['warn', 'schedule', 'Trop de demandes',
      'Rien n’a été fait. Ce n’est pas un plafond du relais : la limite de débit se lève en quelques secondes.']
  };

  function msgBox(state, extra) {
    var m = STEP_MSG[state];
    var box = $('su-msg');
    clear(box);
    box.className = 'su-msg' + (m && m[0] ? ' ' + m[0] : '');
    var strong = el('strong');
    if (m) {
      strong.appendChild(ico(m[1]));
      strong.appendChild(document.createTextNode(m[2]));
      box.appendChild(strong);
      box.appendChild(el('p', null, m[3]));
    }
    if (state === 'refused') {
      var tries = el('div', 'tries');
      tries.setAttribute('aria-hidden', 'true');
      for (var i = 1; i <= 5; i++) {
        tries.appendChild(el('span', i <= refusals ? 'used' : ''));
      }
      box.appendChild(tries);
      box.appendChild(el('p', null, 'Refus ' + refusals + ' sur 5 : au cinquième, cette session se ferme. Ta clé d’accès, elle, reste valable.'));
    }
    if (state === 'throttled' && extra && extra.wait) {
      box.appendChild(el('p', null, 'Réessaie dans ' + fmtDuration(extra.wait) + '.'));
    }
    if (state === 'limit' && extra) {
      var s2 = el('strong');
      s2.appendChild(ico('schedule'));
      s2.appendChild(document.createTextNode('Limite atteinte : ' + extra.cap));
      box.className = 'su-msg warn';
      box.appendChild(s2);
      box.appendChild(el('p', null, extra.next + ' Rien n’a été fait. Si c’est urgent, fais-le sur le serveur :'));
    }
  }

  function setStep(state, extra) {
    su.setAttribute('data-state', state);
    clearTimeout(stepTimer);
    var ok = $('su-ok');
    var cancel = $('su-cancel');
    cancel.textContent = state === 'review' || state === 'pending' ? 'Annuler' : 'Fermer';
    ok.className = 'btn ' + (stepCfg.tone === undefined ? 'danger' : stepCfg.tone);
    ok.querySelector('svg').removeAttribute('hidden');
    $('su-input').disabled = state === 'pending';
    $('su-cli').open = state === 'limit';
    if (state === 'review') {
      textOf('su-ok-label', stepCfg.okLabel);
      syncTyped();
      return;
    }
    if (state === 'pending') {
      // The authenticator window covers the page: no countdown, only "this challenge stays valid
      // 2 min" and, past that, the state "expired".
      stepTimer = setTimeout(function () {
        abortStep();
        setStep('expired');
      }, 120000);
      cancel.focus();
      return;
    }
    msgBox(state, extra);
    textOf('su-ok-label', state === 'expired' ? 'Recommencer' : state === 'error' || state === 'changed' || state === 'lastdevice' || state === 'devicekey' ? 'Actualiser' :
      state === 'closed' ? 'Revenir à la connexion' : 'Réessayer');
    ok.disabled = state === 'limit';
    if (state === 'limit') {
      ok.hidden = true;
    }
    if (state === 'error' || state === 'closed' || state === 'changed' || state === 'lastdevice' || state === 'devicekey') {
      ok.className = 'btn';
      ok.querySelector('svg').setAttribute('hidden', '');
    }
    (state === 'closed' ? ok : cancel).focus();
  }

  function syncTyped() {
    var ok = $('su-ok');
    if (!stepCfg || !stepCfg.expect) {
      ok.disabled = false;
      return;
    }
    var value = $('su-input').value.trim().toLowerCase();
    var good = value === stepCfg.expect;
    ok.disabled = !good;
    var out = $('su-match');
    out.className = 'caption' + (good ? ' ok' : '');
    out.textContent = good ? 'Identifiant correct' : value.length === 8 ? 'Ce n’est pas cet espace' : '';
    $('su-input').setAttribute('aria-invalid', value.length === 8 && !good ? 'true' : 'false');
  }

  function abortStep() {
    if (stepAbort) {
      try {
        stepAbort.abort();
      } catch (e) {
        // Nothing to abort.
      }
      stepAbort = null;
    }
  }

  function closeStep() {
    clearTimeout(stepTimer);
    abortStep();
    if (su.open) {
      su.close();
    }
  }

  // The summary of the sheet comes from `summary` of api/stepup/options.
  function fillRecap(cfg) {
    var recap = $('su-recap');
    clear(recap);
    recap.className = 'recap' + (cfg.facts ? ' facts' : '');
    recap.hidden = !cfg.recap || !cfg.recap.length;
    (cfg.recap || []).forEach(function (row) {
      recap.appendChild(row);
    });
    var cq = $('su-conseq');
    clear(cq);
    cfg.conseq.forEach(function (c) {
      cq.appendChild(el('li', c[1], c[0]));
    });
  }

  function openStep(cfg) {
    stepCfg = cfg;
    ensureCaps();
    textOf('su-title', cfg.title);
    $('su-ico').className = 'su-ico' + (cfg.tone === '' ? ' create' : '');
    clear($('su-ico'));
    $('su-ico').appendChild(ico(cfg.icon, 'lg'));
    fillRecap(cfg);
    $('su-type').hidden = !cfg.expect;
    $('su-input').value = '';
    textOf('su-expect', cfg.expect || '');
    textOf('su-pk-text', cfg.pk);
    textOf('su-cmd', cfg.cmd);
    textOf('su-cmd-note', cfg.cmdNote || '');
    $('su-cmd-note').hidden = !cfg.cmdNote;
    textOf('su-rp', new URL(window.location.href).hostname);
    $('su-ok').hidden = false;
    stepFocus = cfg.from || document.activeElement;
    su.showModal();
    setStep('review');
    if (cfg.direct) {
      beginStep();
    } else {
      $('su-cancel').focus();
    }
  }

  // The server checks the target and the ceiling BEFORE it makes a challenge, then the passkey is
  // asked. Whatever the sheet shows was built by the server from its own data.
  function beginStep() {
    var cfg = stepCfg;
    setStep('pending');
    var aborted = false;
    stepAbort = typeof AbortController === 'function' ? new AbortController() : null;
    api('POST', 'api/stepup/options', { action: cfg.action() })
      .then(function (r) {
        if (!su.open) {
          aborted = true;
          return null;
        }
        if (r.status === 429 || r.status === 409) {
          throw { resp: r };
        }
        if (r.status === 401) {
          throw new Error('session');
        }
        if (r.status !== 200 || !r.data || !r.data.publicKey) {
          throw new Error('failed');
        }
        if (cfg.summary) {
          cfg.summary(r.data.summary || {});
        }
        var pk = r.data.publicKey;
        pk.challenge = fromB64u(pk.challenge);
        (pk.allowCredentials || []).forEach(function (c) {
          c.id = fromB64u(c.id);
        });
        var options = { publicKey: pk };
        if (stepAbort) {
          options.signal = stepAbort.signal;
        }
        return navigator.credentials.get(options).then(function (credential) {
          return { credential: credential };
        });
      })
      .then(function (got) {
        if (!got || !su.open) {
          return null;
        }
        var res = got.credential.response;
        return api('POST', 'api/actions', {
          action: cfg.action(),
          assertion: {
            id: toB64u(got.credential.rawId),
            clientDataJSON: toB64u(res.clientDataJSON),
            authenticatorData: toB64u(res.authenticatorData),
            signature: toB64u(res.signature)
          }
        });
      })
      .then(function (r) {
        if (!r || !su.open) {
          return;
        }
        clearTimeout(stepTimer);
        if (r.status === 200) {
          refusals = 0;
          closeStep();
          cfg.done(r.data || {});
          return;
        }
        var d = r.data || {};
        if (r.status === 401 && d.reason === 'expired') {
          setStep('expired');
        } else if (r.status === 401 && d.session_closed) {
          lostSession('Ta session a été fermée par prudence : 5 confirmations par clé d’accès ont été refusées. Ta clé reste valable.', 'error');
        } else if (r.status === 401 && d.reason === 'refused') {
          refusals = Number(d.refusals) || refusals + 1;
          setStep('refused');
        } else if (r.status === 401) {
          throw new Error('session');
        } else if (r.status === 429 || r.status === 409) {
          stepFromStatus(r);
        } else if (r.status === 400) {
          setStep('error');
        } else {
          setStep('error');
        }
      })
      .catch(function (err) {
        if (aborted || !su.open) {
          return;
        }
        clearTimeout(stepTimer);
        if (err && err.resp) {
          stepFromStatus(err.resp);
        } else if (err && err.message === 'session') {
          lostSession();
        } else if (err && (err.name === 'NotAllowedError' || err.name === 'AbortError')) {
          // Cancelled on the device: nothing was sent to the relay, nothing is counted.
          setStep('cancelled');
        } else {
          setStep('error');
        }
      });
  }

  // A 429 is a ceiling of the relay only if the server says so (`ceiling`); otherwise it is the rate
  // limit of the address, which lifts in seconds. A 409 is "the target changed" unless the server
  // gives its reason.
  function stepFromStatus(r) {
    var d = r.data || {};
    if (r.status === 429) {
      if (d.reason === 'ceiling' || d.ceiling) {
        setStep('limit', limitText(d));
      } else {
        setStep('throttled', { wait: Number(d.retry_after) || 0 });
      }
      return;
    }
    if (d.reason === 'last_device') {
      setStep('lastdevice');
    } else if (d.reason === 'device_key_required') {
      setStep('devicekey');
    } else {
      setStep('changed');
    }
  }

  var CAPS = {
    space_deletions: 'suppressions d’espace',
    device_removals: 'retraits d’appareil',
    invitations: 'invitations'
  };

  function capLine(kind) {
    var c = data.about && data.about.ceilings && data.about.ceilings[kind];
    if (!c) {
      return '';
    }
    var per = c.window_secs >= 86400 ? ' par 24 h' : ' par heure';
    return c.max + ' ' + CAPS[kind] + per + ' au plus sur ce relais';
  }

  function limitText(d) {
    var kind = d.ceiling;
    var cap = kind && CAPS[kind] ? d.max + ' ' + CAPS[kind] + (d.window_secs >= 86400 ? ' par 24 h' : ' par heure') : 'plafond du relais';
    var wait = Number(d.retry_after) || 0;
    if (d.cli) {
      textOf('su-cmd', d.cli);
    }
    return {
      cap: cap,
      next: wait ? 'La prochaine sera possible dans ' + fmtDuration(wait) + '.' : ''
    };
  }

  function ensureCaps() {
    // The ceilings come with api/about; nothing else to prepare.
  }

  su.addEventListener('close', function () {
    clearTimeout(stepTimer);
    abortStep();
    if (stepFocus && document.contains(stepFocus)) {
      stepFocus.focus();
    }
  });

  function onStepOk() {
    var state = su.getAttribute('data-state');
    if (state === 'error' || state === 'changed') {
      closeStep();
      refreshAll();
    } else if (state === 'closed') {
      closeStep();
      lostSession('');
    } else {
      beginStep();
    }
  }

  // ----- what is shown in the sheet, per action -----------------------------------------------

  function recapDevice(d) {
    var li = el('li');
    li.appendChild(ico(platformIcon(d.platform)));
    var main = el('span', 'r-main');
    main.appendChild(el('strong', null, d.name || 'Appareil'));
    main.appendChild(el('span', 'caption', deviceSub(d)));
    li.appendChild(main);
    li.appendChild(el('span', 'mono r-id', d.id));
    return li;
  }

  function recapFact(label, value) {
    var li = el('li');
    li.appendChild(el('span', null, label));
    li.appendChild(el('span', null, value));
    return li;
  }

  function askRevoke(ids, from) {
    var id = selectedSpace;
    var devices = data.devices.filter(function (d) {
      return ids.indexOf(d.id) !== -1;
    });
    if (!devices.length) {
      return;
    }
    var one = devices.length === 1;
    openStep({
      from: from,
      icon: 'person-remove',
      title: one ? 'Retirer ' + (devices[0].name || 'cet appareil') + ' de l’espace ' + id + ' ?' :
        'Retirer ' + devices.length + ' appareils de l’espace ' + id + ' ?',
      recap: devices.map(recapDevice),
      conseq: [
        [one ? 'Il perd l’accès tout de suite. Pour revenir, il devra être recouplé depuis un autre appareil.' :
          'Ils perdent l’accès tout de suite. Pour revenir, ils devront être recouplés depuis un autre appareil.', 'strong'],
        [one ? 'Ses données restent lisibles sur lui : un retrait n’efface rien.' :
          'Leurs données restent lisibles sur eux : un retrait n’efface rien.', ''],
        ['Un appareil restant renouvellera la clé de l’espace à sa prochaine connexion.', 'good']
      ],
      pk: 'Ta clé d’accès te sera demandée · ' + capLine('device_removals'),
      okLabel: 'Retirer avec ma clé d’accès',
      cmd: devices.map(function (d) { return 'tomi-relay admin revoke --space ' + id + ' ' + d.id; }).join('\n'),
      cmdNote: 'Sur le serveur, la commande agit tout de suite, sans autre confirmation.',
      action: function () {
        return { type: 'revoke_devices', space_id: id, device_ids: ids.slice() };
      },
      done: function (r) {
        var removed = r.removed || [];
        say((removed.length === 1 ? (removed[0].name || 'Appareil') + ' retiré' : removed.length + ' appareils retirés') +
          ' · un appareil restant renouvellera la clé');
        refreshAll();
      }
    });
  }

  function askDelete(from) {
    var id = selectedSpace;
    var s = spaceById(id);
    if (!s) {
      return;
    }
    openStep({
      from: from,
      icon: 'delete',
      title: 'Supprimer l’espace ' + id + ' ?',
      facts: true,
      expect: id,
      recap: [
        recapFact('Appareils', plural(s.members, 'appareil', 'appareils') + ', dont ' + s.active + ' actif' + (s.active > 1 ? 's' : '')),
        recapFact('Contenu', plural(s.items, 'élément', 'éléments') + ' · ' + fmtBytes(s.used_bytes)),
        recapFact('Dernière activité', s.last_activity_at ? rel(s.last_activity_at) : 'jamais'),
        recapFact('Créé', fmtDate(s.created_at).split(' ')[0])
      ],
      conseq: [
        [(s.members > 1 ? 'Ses ' + s.members + ' appareils ne le trouvent plus' : 'Son appareil ne le trouve plus') +
          ', tout de suite : plus aucune synchronisation.', 'strong'],
        ['Rien n’est effacé sur les appareils, et aucune invitation n’est créée à la place.', ''],
        ['Le relais garde le dossier chiffré 7 jours. Seul le serveur peut le restaurer.', 'good']
      ],
      pk: 'Ta clé d’accès te sera demandée · ' + capLine('space_deletions'),
      okLabel: 'Supprimer avec ma clé d’accès',
      cmd: 'tomi-relay admin reset --space ' + id + ' --confirm ' + id,
      cmdNote: 'Attention : sur le serveur, l’effacement est immédiat, sans corbeille.',
      action: function () {
        return { type: 'delete_space', space_id: id, confirm: $('su-input').value.trim().toLowerCase() };
      },
      done: function (r) {
        var until = r.trash && r.trash.purge_after ? fmtDate(r.trash.purge_after).split(' ')[0] : '';
        textOf('banner-id', id);
        textOf('banner-until', until);
        textOf('banner-cmd', r.restore || 'tomi-relay admin trash restore --space ' + id);
        $('spaces-banner').hidden = false;
        window.location.hash = '#espaces';
        refreshAll().then(function () {
          $('spaces-banner').focus();
        });
      }
    });
  }

  function askInvite(mode, from) {
    var rotate = mode === 'rotate';
    var pending = data.invitations.length;
    openStep({
      from: from,
      direct: !rotate,
      tone: '',
      icon: rotate ? 'invite' : 'add',
      title: rotate ? 'Remplacer les invitations en attente ?' : 'Créer une invitation',
      recap: rotate ? data.invitations.map(function (i) {
        var li = el('li');
        li.appendChild(ico('invite'));
        var main = el('span', 'r-main');
        main.appendChild(el('strong', null, 'Invitation'));
        main.appendChild(el('span', 'caption', 'créée ' + rel(i.created_at)));
        li.appendChild(main);
        li.appendChild(el('span', 'mono r-id', i.id));
        return li;
      }) : [],
      conseq: rotate ?
        [[(pending > 1 ? 'Les ' + pending + ' invitations en attente cesseront' : 'L’invitation en attente cessera') + ' de fonctionner tout de suite.', 'strong'],
          ['Un nouveau jeton sera créé et affiché une seule fois.', ''],
          ['Les espaces existants ne changent pas.', 'good']] :
        [['Son jeton permettra à qui l’a de créer un espace sur ton relais. Il s’affichera une seule fois.', '']],
      pk: 'Ta clé d’accès te sera demandée · ' + capLine('invitations'),
      okLabel: rotate ? 'Remplacer avec ma clé d’accès' : 'Créer avec ma clé d’accès',
      cmd: rotate ? 'tomi-relay admin invite --rotate' : 'tomi-relay admin invite --new',
      action: function () {
        return { type: 'create_invitation', mode: mode };
      },
      done: function (r) {
        data.invitations = r.invitations || [];
        renderInvitations();
        revealToken(r);
        say(rotate ? 'Invitations remplacées' : 'Invitation créée');
        refreshStatusOnly();
      }
    });
  }

  // ===== overview ==================================================================================

  function fcmLabel(fcm) {
    var s = (fcm && fcm.state) || 'unknown';
    if (s === 'disabled') {
      return ['Désactivées', 'jamais configurées'];
    }
    if (s === 'ok') {
      return ['En service', fcm.last_success_at ? 'dernier envoi ' + rel(fcm.last_success_at) : ''];
    }
    if (s === 'auth_error') {
      return ['Refusées', 'compte de service rejeté'];
    }
    if (s === 'degraded') {
      return ['Dégradées', fcm.last_error ? 'dernière erreur ' + rel(fcm.last_error.at) : ''];
    }
    return ['Inconnu', 'aucun envoi encore'];
  }

  function tile(parent, label, value, sub, o) {
    o = o || {};
    var node = el(o.href ? 'a' : 'div', 'tile' + (o.wide ? ' wide' : ''));
    if (o.href) {
      node.href = o.href;
    }
    var k = el('span', 'k', label);
    if (o.right) {
      k.appendChild(el('span', null, o.right));
    }
    node.appendChild(k);
    var v = el('span', 'v' + (o.word ? ' word' : ''), value);
    if (o.small) {
      v.appendChild(document.createTextNode(' '));
      v.appendChild(el('small', null, o.small));
    }
    node.appendChild(v);
    if (sub) {
      node.appendChild(el('span', 'sub', sub));
    }
    if (o.gauge !== undefined && o.gauge !== null) {
      var p = el('progress');
      p.max = 100;
      p.value = o.gauge;
      p.className = o.gauge >= 100 ? 'full' : o.gauge >= 90 ? 'warn' : '';
      p.setAttribute('aria-label', 'Stockage utilisé : ' + fmtPercent(o.gauge));
      p.textContent = fmtPercent(o.gauge);
      node.appendChild(p);
    }
    parent.appendChild(node);
  }

  function refusedCount() {
    var n = 0;
    var last = null;
    ((data.journal && data.journal.entries) || []).forEach(function (e) {
      if (e.kind === 'sign_in_refused' || e.kind === 'enrollment_refused') {
        n += Number(e.count || 1);
        if (!last || Date.parse(e.at) > Date.parse(last)) {
          last = e.at;
        }
      }
    });
    return { count: n, last: last };
  }

  function alertList() {
    var list = [];
    var s = data.status;
    data.spaces.forEach(function (sp) {
      var p = percent(sp.used_bytes, sp.max_bytes);
      if (p !== null && p >= 90) {
        list.push({
          level: p >= 100 ? 'bad' : 'warn',
          who: 'Espace ' + sp.id,
          mono: true,
          text: p >= 100 ? ' : quota atteint, ses envois sont refusés.' : ' : ' + fmtPercent(p) + ' de son quota utilisé.',
          href: '#espaces/' + sp.id,
          label: 'Voir'
        });
      }
    });
    if (s && s.quota) {
      var rp = percent(s.quota.used_bytes, s.quota.max_bytes);
      if (rp !== null && rp >= 90) {
        list.push({ level: rp >= 100 ? 'bad' : 'warn', who: 'Stockage', text: ' : ' + fmtPercent(rp) + ' du total du relais utilisé.', href: '#reglages', label: 'Réglages' });
      }
    }
    if (s && data.about && data.about.limits) {
      var floor = Number(data.about.limits.min_free_disk_bytes) || 0;
      if (floor && Number(s.disk_free_bytes) < floor * 2) {
        list.push({ level: Number(s.disk_free_bytes) < floor ? 'bad' : 'warn', who: 'Disque', text: ' : ' + fmtBytes(s.disk_free_bytes) + ' libres, le relais en garde ' + fmtBytes(floor) + '.', href: '#reglages', label: 'Réglages' });
      }
    }
    if (s && s.fcm && s.fcm.state === 'auth_error') {
      list.push({ level: 'bad', who: 'Notifications', text: ' : le compte de service est refusé.', href: '#reglages', label: 'Réglages' });
    } else if (s && s.fcm && s.fcm.state === 'degraded') {
      list.push({ level: 'warn', who: 'Notifications', text: ' : envois dégradés.', href: '#reglages', label: 'Réglages' });
    }
    var rf = refusedCount();
    if (rf.count > 0) {
      list.push({
        level: 'warn',
        who: 'Connexions',
        text: ' : ' + rf.count + ' tentative' + (rf.count > 1 ? 's' : '') + ' refusée' + (rf.count > 1 ? 's' : '') + (rf.last ? ', la dernière ' + rel(rf.last) : '') + '.',
        href: '#journal',
        label: 'Journal'
      });
    }
    return list;
  }

  function renderStatus() {
    var s = data.status;
    var box = $('status-list');
    clear(box);
    if (!s) {
      return;
    }
    var list = alertList();
    var worst = list.some(function (a) { return a.level === 'bad'; }) ? 'bad' : list.length ? 'warn' : 'good';
    $('status-box').className = 'status' + (worst === 'good' ? '' : ' ' + worst);
    setIcon($('status-use').parentNode, worst === 'good' ? 'check-circle' : worst === 'bad' ? 'error' : 'warning');
    textOf('h-status', worst === 'good' ? 'Rien à signaler' : worst === 'bad' ?
      plural(list.length, 'problème', 'problèmes') : plural(list.length, 'point d’attention', 'points d’attention'));
    var sub = 'Relais v' + s.relay_version;
    if (data.about) {
      sub += ' · en service depuis ' + fmtDuration(data.about.uptime_secs);
    }
    textOf('status-sub', sub);
    var ul = $('alerts');
    clear(ul);
    list.forEach(function (a) {
      var li = el('li', a.level === 'bad' ? 'bad' : '');
      li.appendChild(ico(a.level === 'bad' ? 'error' : 'warning'));
      var span = el('span');
      var strong = el('strong');
      strong.appendChild(document.createTextNode(a.who.replace(/ [0-9a-f]{8}$/, ' ')));
      if (a.mono) {
        strong.appendChild(el('span', 'mono', a.who.slice(-8)));
      }
      span.appendChild(strong);
      span.appendChild(document.createTextNode(a.text));
      li.appendChild(span);
      var link = el('a', 'btn ghost sm', a.label);
      link.href = a.href;
      li.appendChild(link);
      ul.appendChild(li);
    });
    ul.hidden = !list.length;

    var p = percent(s.quota.used_bytes, s.quota.max_bytes);
    tile(box, 'Stockage, tous les espaces', fmtBytes(s.quota.used_bytes), null, {
      wide: true,
      right: p === null ? '' : fmtPercent(p),
      small: s.quota.max_bytes ? 'sur ' + fmtBytes(s.quota.max_bytes) : '',
      gauge: p
    });
    var members = 0;
    var active = 0;
    data.spaces.forEach(function (sp) {
      members += Number(sp.members || 0);
      active += Number(sp.active || 0);
    });
    tile(box, 'Espaces', String(s.spaces), plural(members, 'appareil', 'appareils') + ', ' + active + ' actif' + (active > 1 ? 's' : ''), { href: '#espaces' });
    tile(box, 'Invitations', String(s.invitations_pending), 'en attente', { href: '#invitations' });
    var f = fcmLabel(s.fcm);
    tile(box, 'Notifications (FCM)', f[0], f[1], { word: true });
    tile(box, 'Disque libre', fmtBytes(s.disk_free_bytes), 'volume des données');
  }

  // ===== spaces ====================================================================================

  function deviceSub(d) {
    var parts = [];
    if (d.platform) {
      parts.push(platformLabel(d.platform));
    }
    if (d.app_version) {
      parts.push(d.app_version);
    }
    parts.push(d.last_seen_at ? rel(d.last_seen_at) : 'jamais vu');
    return parts.join(' · ');
  }

  function platformLabel(p) {
    return { windows: 'Windows', android: 'Android', ios: 'iOS', macos: 'macOS', linux: 'Linux' }[p] || String(p || '');
  }

  function platformIcon(p) {
    return p === 'android' || p === 'ios' ? 'phone' : 'computer';
  }

  function spaceBadges(s) {
    var out = [];
    var p = percent(s.used_bytes, s.max_bytes);
    if (p !== null && p >= 100) {
      out.push(badge('Quota atteint', 'bad'));
    } else if (p !== null && p >= 90) {
      out.push(badge('Quota à ' + Math.round(p) + ' %', 'warn'));
    }
    if (s.rotation_required) {
      out.push(badge('Clé à renouveler', 'warn'));
    }
    if (!s.active) {
      out.push(badge('Inactif', 'muted'));
    }
    if (!out.length) {
      out.push(badge('Normal', 'good'));
    }
    return out;
  }

  function sortValue(s, key) {
    switch (key) {
      case 'id': return s.id;
      case 'devices': return Number(s.members || 0);
      case 'items': return Number(s.items || 0);
      case 'usage': return percent(s.used_bytes, s.max_bytes) === null ? Number(s.used_bytes) / 1e12 : percent(s.used_bytes, s.max_bytes);
      case 'created': return Date.parse(s.created_at) || 0;
      default: return s.last_activity_at ? Date.parse(s.last_activity_at) : 0;
    }
  }

  function sortedBy(list, sort, valueOf) {
    return list.slice().sort(function (a, b) {
      var x = valueOf(a, sort.key);
      var y = valueOf(b, sort.key);
      if (x < y) {
        return -sort.dir;
      }
      if (x > y) {
        return sort.dir;
      }
      return 0;
    });
  }

  function timeCell(text) {
    return timeEl(text);
  }

  function renderSpaces() {
    var body = $('spaces-body');
    clear(body);
    var rows = sortedBy(data.spaces, sorts.spaces, sortValue);
    textOf('spaces-count', plural(rows.length, 'espace', 'espaces'));
    $('spaces-empty').hidden = rows.length > 0;
    $('spaces-table').hidden = rows.length === 0;
    $('spaces-sort').value = sorts.spaces.key;
    Array.prototype.forEach.call(document.querySelectorAll('#spaces-table button.sort'), function (b) {
      var on = b.getAttribute('data-sort') === sorts.spaces.key;
      var th = b.closest('th');
      if (on) {
        th.setAttribute('aria-sort', sorts.spaces.dir > 0 ? 'ascending' : 'descending');
      } else {
        th.removeAttribute('aria-sort');
      }
      b.querySelector('.dir').textContent = on ? (sorts.spaces.dir > 0 ? '▲' : '▼') : '';
    });
    rows.forEach(function (s) {
      var tr = el('tr');
      tr.setAttribute('data-space', s.id);
      var th = el('th', 'c-id');
      th.setAttribute('scope', 'row');
      var a = el('a', 'space-link mono', s.id);
      a.href = '#espaces/' + s.id;
      th.appendChild(a);
      var copy = copyBtn('Identifiant', 'Copier l’identifiant ' + s.id);
      th.appendChild(copy);
      tr.appendChild(th);

      var st = el('td', 'c-state');
      var badges = el('span', 'badges');
      spaceBadges(s).forEach(function (b) { badges.appendChild(b); });
      st.appendChild(badges);
      tr.appendChild(st);

      var dev = el('td', 'c-dev');
      dev.setAttribute('data-label', 'Appareils');
      dev.appendChild(el('span', 'v', s.active + ' actif' + (s.active > 1 ? 's' : '') + ' sur ' + s.members));
      dev.appendChild(el('span', 'sub', s.present + ' présent' + (s.present > 1 ? 's' : '')));
      tr.appendChild(dev);

      var items = el('td', 'c-items');
      items.setAttribute('data-label', 'Contenu');
      items.appendChild(el('span', 'v', plural(s.items, 'élément', 'éléments')));
      items.appendChild(el('span', 'sub', plural(s.blobs, 'fichier chiffré', 'fichiers chiffrés')));
      tr.appendChild(items);

      var usage = el('td', 'c-usage');
      var p = percent(s.used_bytes, s.max_bytes);
      if (p !== null) {
        var pr = el('progress');
        pr.max = 100;
        pr.value = p;
        pr.className = p >= 100 ? 'full' : p >= 90 ? 'warn' : '';
        pr.setAttribute('aria-label', 'Quota utilisé : ' + fmtPercent(p));
        pr.textContent = fmtPercent(p);
        usage.appendChild(pr);
      }
      var txt = el('div', 'usage-txt');
      var left = el('span');
      left.appendChild(el('strong', null, fmtBytes(s.used_bytes)));
      left.appendChild(document.createTextNode(s.max_bytes ? ' sur ' + fmtBytes(s.max_bytes) : ' utilisés'));
      txt.appendChild(left);
      txt.appendChild(el('span', null, p === null ? 'aucune limite' : fmtPercent(p)));
      usage.appendChild(txt);
      tr.appendChild(usage);

      var act = el('td', 'c-act');
      act.appendChild(timeCell(s.last_activity_at));
      tr.appendChild(act);
      var crt = el('td', 'c-crt');
      crt.appendChild(timeCell(s.created_at));
      tr.appendChild(crt);

      var go = el('td', 'c-go');
      var link = el('a', 'btn tonal sm');
      link.href = '#espaces/' + s.id;
      link.setAttribute('aria-label', 'Détail de l’espace ' + s.id);
      link.appendChild(el('span', null, 'Détail'));
      link.appendChild(ico('chevron'));
      go.appendChild(link);
      tr.appendChild(go);
      if (s.id === selectedSpace) {
        tr.className = 'selected';
      }
      body.appendChild(tr);
    });
  }

  function setSort(name, key) {
    var sort = sorts[name];
    if (sort.key === key) {
      sort.dir = -sort.dir;
    } else {
      sort.key = key;
      sort.dir = key === 'id' || key === 'name' || key === 'platform' ? 1 : -1;
    }
    if (name === 'spaces') {
      renderSpaces();
    } else {
      renderDevices();
    }
  }

  // ----- the detail of a space ---------------------------------------------------------------------

  function quotaOriginLine(s) {
    var d = data.status && data.status.default_space_quota;
    if (s.quota_source === 'default') {
      return !d || d.bytes === null ? 'Quota par défaut : aucune limite' : 'Quota par défaut (' + fmtBytes(d.bytes) + ')';
    }
    return s.max_bytes === null ? 'Sans limite, propre à cet espace' : 'Quota propre à cet espace';
  }

  function pairInto(id, main, sub) {
    var node = $(id);
    node.textContent = main;
    if (sub) {
      node.appendChild(el('span', 'sub', sub));
    }
  }

  function renderDetail() {
    var s = spaceById(selectedSpace);
    var open = !!s;
    $('space-detail').hidden = !open;
    document.body.classList.toggle('has-detail', open);
    $('spaces-layout').classList.toggle('has-detail', open);
    if (!s) {
      return;
    }
    textOf('d-id', s.id);
    textOf('t-space', s.id);
    var badges = $('d-badges');
    clear(badges);
    spaceBadges(s).forEach(function (b) { badges.appendChild(b); });
    pairInto('d-dev', s.active + ' actif' + (s.active > 1 ? 's' : '') + ' sur ' + s.members, s.present + ' présent' + (s.present > 1 ? 's' : '') + ' en ce moment');
    pairInto('d-items', plural(s.items, 'élément', 'éléments'), plural(s.blobs, 'fichier chiffré', 'fichiers chiffrés'));
    var retention = data.about && data.about.limits ? data.about.limits.event_retention_days : null;
    pairInto('d-events', plural(s.events, 'événement', 'événements'), retention ? 'retenus ' + retention + ' j au plus' : '');
    pairInto('d-created', fmtDate(s.created_at).split(' ')[0], rel(s.created_at));
    var p = percent(s.used_bytes, s.max_bytes);
    var pg = $('d-progress');
    pg.hidden = p === null;
    if (p !== null) {
      pg.value = p;
      pg.className = p >= 100 ? 'full' : p >= 90 ? 'warn' : '';
      pg.setAttribute('aria-label', 'Quota utilisé : ' + fmtPercent(p));
    }
    var usage = $('d-usage');
    clear(usage);
    usage.appendChild(el('strong', null, fmtBytes(s.used_bytes)));
    usage.appendChild(document.createTextNode(s.max_bytes ? ' sur ' + fmtBytes(s.max_bytes) : ' utilisés'));
    textOf('d-pct', p === null ? 'aucune limite' : fmtPercent(p));
    textOf('d-quota', quotaOriginLine(s) + (p === null ? '' : p >= 100 ? ' · ses envois sont refusés' : p >= 90 ? ' · au-delà, les envois sont refusés' : ''));
    $('sq-default').hidden = s.quota_source === 'default';

    // The key of the space: the relay does not have it, so there is no button, only the reason.
    var rot = $('d-rotation');
    clear(rot);
    rot.className = 'note-line' + (s.rotation_required ? ' warn' : '');
    rot.appendChild(ico(s.rotation_required ? 'warning' : 'check'));
    var span = el('span');
    if (s.rotation_required) {
      span.appendChild(el('strong', null, 'Clé à renouveler'));
      span.appendChild(document.createTextNode(' après un retrait. Un appareil restant le fait tout seul à sa prochaine connexion ; le relais, qui n’a pas la clé, ne peut ni le faire ni le valider à sa place.'));
    } else {
      span.textContent = 'Clé de l’espace : rien en attente';
    }
    rot.appendChild(span);
    renderCommands(s);
  }

  function renderCommands(s) {
    var ul = $('d-commands');
    clear(ul);
    var first = data.devices[0] ? data.devices[0].id : 'ID_APPAREIL';
    [
      ['État détaillé de l’espace', 'tomi-relay admin status --space ' + s.id],
      ['Appareils, avec leur identifiant complet', 'tomi-relay admin devices --space ' + s.id],
      ['Quota propre à l’espace (OCTETS, none ou default)', 'tomi-relay admin quota --space ' + s.id + ' OCTETS'],
      ['Retirer un appareil', 'tomi-relay admin revoke --space ' + s.id + ' ' + first],
      ['Supprimer l’espace (immédiat, sans corbeille)', 'tomi-relay admin reset --space ' + s.id + ' --confirm ' + s.id]
    ].forEach(function (c) {
      ul.appendChild(cmdRow(c[0], c[1]));
    });
  }

  function cmdRow(what, command) {
    var li = el('li');
    li.appendChild(el('span', 'what', what));
    var div = el('div', 'cmd');
    div.appendChild(el('code', null, command));
    div.appendChild(copyBtn('Commande', 'Copier la commande : ' + what));
    li.appendChild(div);
    return li;
  }

  function deviceState(d) {
    return d.present ? ['Présent', 'good'] : d.active ? ['Actif', ''] : ['Inactif', 'muted'];
  }

  function devValue(d, key) {
    switch (key) {
      case 'name': return (d.name || '').toLowerCase();
      case 'platform': return d.platform || '';
      case 'added': return Date.parse(d.registered_at) || 0;
      default: return d.last_seen_at ? Date.parse(d.last_seen_at) : 0;
    }
  }

  function renderDevices() {
    var list = $('d-devices');
    clear(list);
    var last = data.devices.length === 1;
    textOf('d-count', String(data.devices.length));
    $('devices-sort').value = sorts.devices.key;
    textOf('d-dev-hint', last ?
      'Dernier appareil de l’espace : il ne peut pas être retiré. Pour en finir avec cet espace, supprime-le.' :
      'Coche un ou plusieurs appareils pour les retirer : une seule confirmation avec ta clé d’accès.');
    var tpl = $('tpl-device');
    sortedBy(data.devices, sorts.devices, devValue).forEach(function (d) {
      var node = tpl.content.cloneNode(true);
      var root = node.querySelector('li');
      root.setAttribute('data-device', d.id);
      setIcon(node.querySelector('summary svg'), platformIcon(d.platform));
      node.querySelector('.name').textContent = d.name || 'Appareil';
      node.querySelector('.sub').textContent = deviceSub(d);
      var state = deviceState(d);
      var b = node.querySelector('.badge');
      b.textContent = state[0];
      b.className = 'badge' + (state[1] ? ' ' + state[1] : '');
      node.querySelector('.f-added').textContent = fmtDate(d.registered_at);
      node.querySelector('.f-push').textContent = d.push ? 'Activées' : 'Non';
      node.querySelector('.f-id').textContent = d.id;
      node.querySelector('.f-fp').textContent = d.fingerprint || '';
      node.querySelector('[data-copy]').setAttribute('aria-label', 'Copier l’empreinte de ' + (d.name || 'l’appareil'));
      var box = node.querySelector('.dev-sel');
      box.value = d.id;
      box.disabled = last;
      node.querySelector('.dev-check .sr').textContent = 'Sélectionner ' + (d.name || 'l’appareil');
      node.querySelector('.dev-revoke').hidden = last;
      node.querySelector('.dev-last').hidden = !last;
      list.appendChild(node);
    });
    syncSelection();
  }

  function selectedIds() {
    return Array.prototype.map.call(document.querySelectorAll('#d-devices .dev-sel:checked'), function (b) {
      return b.value;
    });
  }

  // Never a shortcut and never a button under the pointer: a checkbox, then "Retirer".
  function syncSelection() {
    var ids = selectedIds();
    Array.prototype.forEach.call(document.querySelectorAll('#d-devices .dev'), function (li) {
      li.classList.toggle('selected', li.querySelector('.dev-sel').checked);
    });
    var bar = $('d-selbar');
    bar.hidden = ids.length === 0;
    if (!ids.length) {
      return;
    }
    var everyone = ids.length === data.devices.length;
    var count = $('d-selcount');
    count.className = everyone ? 'bad' : '';
    count.textContent = everyone ?
      'Il doit rester au moins un appareil pour renouveler la clé. Pour retirer tout le monde, supprime l’espace.' :
      plural(ids.length, 'appareil coché', 'appareils cochés');
    $('d-selrevoke').disabled = everyone;
  }

  function loadDevices(spaceId) {
    $('devices-error').hidden = true;
    return read('api/devices?space=' + encodeURIComponent(spaceId)).then(
      function (r) {
        if (selectedSpace !== spaceId) {
          return;
        }
        data.devices = r.devices || [];
        renderDevices();
        var s = spaceById(spaceId);
        if (s) {
          renderCommands(s);
        }
      },
      function (err) {
        if (err.message === 'session') {
          lostSession();
          return;
        }
        data.devices = [];
        renderDevices();
        textOf('devices-error', 'Impossible de lire les appareils de cet espace.');
        $('devices-error').hidden = false;
      }
    );
  }

  // ----- the quota of one space (session only; confirmed if the space would be blocked) ----------

  function closeQuotaForm() {
    $('sq-form').hidden = true;
    $('sq-edit').hidden = false;
    $('sq-edit').setAttribute('aria-expanded', 'false');
  }

  function mibToBytes(text) {
    var t = String(text).trim().replace(',', '.');
    if (!/^[0-9]+(\.[0-9]+)?$/.test(t)) {
      return null;
    }
    return Math.round(Number(t) * 1048576);
  }

  function previewSpaceQuota() {
    var s = spaceById(selectedSpace);
    var out = $('sq-preview');
    var bytes = mibToBytes($('sq-mib').value);
    out.className = 'caption preview';
    if (!s || bytes === null || bytes < 1048576) {
      out.textContent = $('sq-mib').value ? 'Au moins 1 Mio.' : '';
      if ($('sq-mib').value) {
        out.classList.add('bad');
      }
      return null;
    }
    if (Number(s.used_bytes) > bytes) {
      out.classList.add('warn');
      out.textContent = 'L’espace utilise déjà ' + fmtBytes(s.used_bytes) + ' : ses envois seraient refusés.';
    } else {
      out.textContent = '= ' + fmtBytes(bytes);
    }
    return bytes;
  }

  function saveSpaceQuota(value, okText) {
    var id = selectedSpace;
    var s = spaceById(id);
    function send(confirm) {
      return api('POST', 'api/spaces/quota', { space_id: id, value: value, confirm: confirm }).then(function (r) {
        if (r.status === 401) {
          lostSession();
          return;
        }
        if (r.status === 409 && r.data && r.data.reason === 'confirmation_required') {
          return confirmAction({
            title: 'Bloquer les envois de cet espace ?',
            text: 'L’espace utilise déjà plus que ce quota : ses envois seront refusés jusqu’à ce qu’il repasse dessous. Rien n’est supprimé.',
            ok: 'Enregistrer quand même',
            list: [[id, 'utilise ' + fmtBytes(s ? s.used_bytes : 0)]]
          }).then(function (yes) {
            return yes ? send(true) : undefined;
          });
        }
        if (r.status !== 200) {
          say('Le quota n’a pas été enregistré.', 'error');
          return;
        }
        closeQuotaForm();
        say(okText);
        return refreshAll();
      });
    }
    if (value === 0) {
      return confirmAction({
        title: 'Retirer la limite de cet espace ?',
        text: 'L’espace pourra occuper tout le disque du relais, dans la limite du total.',
        ok: 'Sans limite'
      }).then(function (yes) {
        return yes ? send(false) : undefined;
      });
    }
    return send(false);
  }

  // ===== invitations ================================================================================

  function renderInvitations() {
    var list = $('invitations-list');
    clear(list);
    var n = data.invitations.length;
    data.invitations.forEach(function (i) {
      var li = el('li');
      var left = el('span', null, 'Invitation ');
      left.appendChild(el('span', 'mono', i.id));
      li.appendChild(left);
      var t = timeEl(i.created_at);
      t.className = 'muted nowrap';
      t.textContent = 'créée ' + t.textContent;
      li.appendChild(t);
      list.appendChild(li);
    });
    list.hidden = !n;
    $('inv-empty').hidden = !!n;
    $('inv-actions').hidden = !n;
    textOf('inv-count', String(n));
    textOf('inv-cap', capLine('invitations').replace(/ sur ce relais$/, ''));
  }

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
      canvases[i].width = 0;
      canvases[i].height = 0;
    }
    clear(holder);
    $('token-qr-wrap').hidden = true;
  }

  // The token is not displayed for ever: 10 minutes at most, and never after leaving the page. It
  // is written nowhere else (neither local storage nor log).
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

  function revealToken(r) {
    var link = r.link || '';
    $('token').textContent = r.token;
    $('token-link').textContent = link;
    $('token-link-wrap').hidden = !link;
    $('token-link-copy').hidden = !link;
    showQr(link || r.token);
    $('token-box').hidden = false;
    window.scrollTo(0, 0);
    $('token-title').focus({ preventScroll: true });
    clearTimeout(tokenTimer);
    tokenTimer = setTimeout(hideToken, TOKEN_SHOWN_MS);
  }

  function cancelInvitations() {
    var list = data.invitations.map(function (i) { return [i.id, 'créée ' + rel(i.created_at)]; });
    confirmAction({
      title: 'Invalider les invitations en attente ?',
      text: 'Ces jetons cesseront de fonctionner. Aucune nouvelle invitation n’est créée, et les espaces existants ne changent pas.',
      ok: 'Invalider',
      list: list
    }).then(function (yes) {
      if (!yes) {
        return;
      }
      api('POST', 'api/invitations/cancel', {}).then(function (r) {
        if (r.status === 401) {
          lostSession();
          return;
        }
        if (r.status !== 200 || !r.data) {
          say('Les invitations n’ont pas été invalidées.', 'error');
          return;
        }
        data.invitations = r.data.invitations || [];
        hideToken();
        renderInvitations();
        say(plural(r.data.cancelled, 'invitation invalidée', 'invitations invalidées'));
        refreshStatusOnly();
      });
    });
  }

  // ===== journal ====================================================================================

  var JOURNAL_TEXT = {
    signed_in: 'Connexion avec la clé d’accès',
    sign_in_refused: 'Connexion refusée',
    enrollment_refused: 'Code d’enrôlement refusé',
    signed_out: 'Déconnexion',
    passkey_registered: 'Clé d’accès enregistrée',
    passkey_added: 'Clé d’accès ajoutée',
    passkey_removed: 'Clé d’accès retirée',
    passkey_reset: 'Clés d’accès réinitialisées',
    invitation_created: 'Invitation créée',
    invitations_replaced: 'Invitations remplacées',
    invitations_cancelled: 'Invitations invalidées',
    default_quota_set: 'Quota par défaut réglé',
    default_quota_unlimited: 'Quota par défaut : sans limite',
    default_quota_config: 'Quota par défaut : retour à la configuration',
    space_quota_set: 'Quota d’un espace réglé',
    space_quota_unlimited: 'Quota d’un espace : sans limite',
    space_quota_default: 'Quota d’un espace : retour au défaut',
    quota_set: 'Quota d’un espace modifié',
    path_rotated: 'Adresse du panneau changée : sessions fermées',
    policy_changed: 'Politique de clé modifiée',
    stepup_refused: 'Confirmation par clé d’accès refusée',
    session_closed_by_refusals: 'Session fermée après 5 refus',
    sessions_closed: 'Toutes les sessions fermées',
    devices_removed: 'Appareils retirés',
    device_removed: 'Appareil retiré',
    space_deleted: 'Espace supprimé, mis à la corbeille',
    space_restored: 'Espace restauré',
    trash_restored: 'Espace restauré depuis la corbeille',
    trash_purged: 'Espace purgé de la corbeille',
    trash_expired: 'Espace effacé de la corbeille (7 jours)'
  };

  var WARN_KINDS = ['sign_in_refused', 'enrollment_refused', 'stepup_refused', 'session_closed_by_refusals'];

  function journalRow(e, kept) {
    var li = el('li', (WARN_KINDS.indexOf(e.kind) !== -1 ? 'warn' : '') + (kept ? ' kept' : ''));
    li.appendChild(timeEl(e.at));
    var what = el('span', 'what');
    var text = JOURNAL_TEXT[e.kind] || e.kind;
    var head = el('span');
    if (WARN_KINDS.indexOf(e.kind) !== -1) {
      head.appendChild(ico('warning', 'sm'));
    }
    head.appendChild(document.createTextNode(text));
    if (e.target) {
      head.appendChild(document.createTextNode(' '));
      head.appendChild(el('span', 'mono', e.target));
    }
    if (e.bytes !== undefined && e.bytes !== null) {
      head.appendChild(document.createTextNode(' : ' + fmtBytes(e.bytes)));
    }
    what.appendChild(head);
    var small = [];
    if (e.count && e.count > 1) {
      small.push(e.count + ' fois, depuis ' + rel(e.first_at).replace(/^il y a /, 'il y a '));
    }
    if (e.key) {
      small.push('clé ' + e.key);
    }
    if (kept) {
      small.push('gardé 90 j');
    }
    if (small.length) {
      what.appendChild(el('small', null, small.join(' · ')));
    }
    li.appendChild(what);
    if (e.kind === 'sign_in_refused' || e.kind === 'enrollment_refused') {
      var adv = el('span', 'advice');
      adv.appendChild(document.createTextNode('Pas toi ? Ferme toutes les sessions depuis Réglages, puis change l’adresse du panneau, sur le serveur : '));
      adv.appendChild(el('code', null, 'tomi-relay admin web --rotate-path'));
      li.appendChild(adv);
    }
    return li;
  }

  function renderJournal() {
    var list = $('journal-list');
    clear(list);
    var j = data.journal;
    if (!j) {
      return;
    }
    // The persistent lines (90 days) and the memory of this start, newest first; a persistent
    // line that the memory also holds is shown once.
    var rows = [];
    (j.audit || []).forEach(function (e) { rows.push([e, true]); });
    var seen = {};
    (j.audit || []).forEach(function (e) { seen[e.kind + '@' + e.at] = true; });
    (j.entries || []).forEach(function (e) {
      if (!seen[e.kind + '@' + e.at]) {
        rows.push([e, false]);
      }
    });
    rows.sort(function (a, b) { return Date.parse(b[0].at) - Date.parse(a[0].at); });
    rows.forEach(function (r) { list.appendChild(journalRow(r[0], r[1])); });
    textOf('journal-count', String(rows.length));
    $('journal-empty').hidden = rows.length > 0;
  }

  // ===== settings ===================================================================================

  function renderQuota() {
    var q = data.quota;
    if (!q) {
      return;
    }
    textOf('quota-now', q.bytes === null ? 'Aucune limite par espace' : fmtBytes(q.bytes) + ' par espace');
    var origin = q.source === 'panel' ? 'Réglé depuis ce panneau' : q.source === 'config' ? 'Réglé par la configuration du relais' : 'Aucune limite réglée';
    textOf('quota-facts', origin + ' · total du relais : ' + fmtBytes(q.relay_max_bytes));
    if (document.activeElement !== $('quota-mib')) {
      $('quota-mib').value = q.bytes === null ? '' : String(Math.round(q.bytes / 1048576 * 10) / 10).replace('.', ',');
    }
    previewQuota();
  }

  function quotaBytes() {
    var b = mibToBytes($('quota-mib').value);
    return b;
  }

  function previewQuota() {
    var out = $('quota-preview');
    var q = data.quota;
    var bytes = quotaBytes();
    out.className = 'caption preview';
    $('quota-mib').setAttribute('aria-invalid', 'false');
    if (!$('quota-mib').value) {
      out.textContent = '';
      return;
    }
    if (bytes === null || !q || bytes < q.min_bytes || bytes > q.max_bytes) {
      out.className = 'caption preview bad';
      out.textContent = 'Entre ' + fmtBytes(q ? q.min_bytes : 1048576) + ' et ' + fmtBytes(q ? q.max_bytes : 0) + '.';
      $('quota-mib').setAttribute('aria-invalid', 'true');
      return;
    }
    out.textContent = '= ' + fmtBytes(bytes) + ' par espace';
  }

  // The blocked spaces are told BEFORE saving (the server asks for the confirmation).
  function saveDefaultQuota(value, okText) {
    function send(confirm) {
      return api('POST', 'api/quota', { default_bytes: value, confirm: confirm }).then(function (r) {
        if (r.status === 401) {
          lostSession();
          return;
        }
        if (r.status === 409 && r.data && r.data.reason === 'confirmation_required') {
          var ids = r.data.blocked || [];
          return confirmAction({
            title: ids.length > 1 ? 'Bloquer les envois de ' + ids.length + ' espaces ?' : 'Bloquer les envois d’un espace ?',
            text: 'Ces espaces utilisent déjà plus que ce quota : leurs envois seront refusés jusqu’à ce qu’ils repassent dessous. Rien n’est supprimé.',
            ok: 'Enregistrer quand même',
            list: ids.map(function (id) { return [id, 'dépasse le quota']; })
          }).then(function (yes) {
            return yes ? send(true) : undefined;
          });
        }
        if (r.status !== 200 || !r.data) {
          say('Le quota n’a pas été enregistré.', 'error');
          return;
        }
        data.quota = r.data.default;
        renderQuota();
        say(okText);
        return refreshAll();
      });
    }
    return send(false);
  }

  function submitQuota(event) {
    event.preventDefault();
    var bytes = quotaBytes();
    var q = data.quota;
    if (bytes === null || !q || bytes < q.min_bytes || bytes > q.max_bytes) {
      previewQuota();
      $('quota-mib').focus();
      return;
    }
    saveDefaultQuota(bytes, 'Quota par défaut réglé : ' + fmtBytes(bytes) + ' par espace.');
  }

  function quotaUnlimited() {
    confirmAction({
      title: 'Retirer la limite par espace ?',
      text: 'Un espace sans quota propre pourra occuper tout le disque du relais, dans la limite du total.',
      ok: 'Sans limite'
    }).then(function (yes) {
      if (yes) {
        saveDefaultQuota(0, 'Quota par défaut : aucune limite par espace.');
      }
    });
  }

  function facts(listId, rows) {
    var dl = $(listId);
    clear(dl);
    rows.forEach(function (r) {
      var div = el('div');
      div.appendChild(el('dt', null, r[0]));
      var dd = el('dd', null, r[1]);
      if (r[2]) {
        dd.appendChild(el('span', 'caption', r[2]));
      }
      div.appendChild(dd);
      dl.appendChild(div);
    });
  }

  function capFact(kind) {
    var c = data.about && data.about.ceilings && data.about.ceilings[kind];
    return c ? c.max + (c.window_secs >= 86400 ? ' par 24 h' : ' par heure') : '';
  }

  function renderAbout() {
    var a = data.about;
    if (!a) {
      return;
    }
    facts('about-facts', [
      ['Version du relais', a.relay_version],
      ['En service depuis', fmtDuration(a.uptime_secs), 'démarré le ' + fmtDate(a.started_at)],
      ['Horloge du serveur', fmtDate(a.server_time).split(' ')[1] || a.server_time, Math.abs(serverOffset) < 5000 ? 'à l’heure de cet appareil' : 'écart de ' + Math.round(serverOffset / 1000) + ' s'],
      ['Notifications push', a.push_configured ? 'Configurées' : 'Non configurées'],
      ['Ta session', fmtDuration(a.session.idle_ttl_secs) + ' sans activité', fmtDuration(a.session.absolute_ttl_secs) + ' au plus après la connexion']
    ]);
    var l = a.limits;
    facts('limits-facts', [
      ['Total du relais', fmtBytes(l.max_relay_bytes)],
      ['Quota par espace (configuration)', l.max_space_bytes ? fmtBytes(l.max_space_bytes) : 'aucun'],
      ['Appareils par espace', String(l.max_devices)],
      ['Élément chiffré', fmtBytes(l.max_envelope_bytes) + ' au plus'],
      ['Fichier', fmtBytes(l.max_blob_bytes) + ' au plus'],
      ['Fichiers d’un élément', fmtBytes(l.max_item_blob_bytes) + ' au total'],
      ['Échéance la plus longue', fmtDuration(l.max_ttl_secs)],
      ['Rétention du journal d’un espace', l.event_retention_days + ' j'],
      ['Appareil compté inactif', 'après ' + l.inactive_after_days + ' j sans connexion'],
      ['Purge des éléments échus', 'toutes les ' + fmtDuration(l.purge_interval_secs)],
      ['Fichier jamais rattaché', 'supprimé après ' + fmtDuration(l.blob_orphan_ttl_secs)],
      ['Session d’un appareil', fmtDuration(l.session_ttl_secs)],
      ['Espace disque gardé libre', fmtBytes(l.min_free_disk_bytes)],
      ['Suppressions d’espace depuis le web', capFact('space_deletions')],
      ['Retraits d’appareil depuis le web', capFact('device_removals')],
      ['Invitations depuis le web', capFact('invitations')]
    ]);
    textOf('access-others', String(a.sessions ? a.sessions.others : 0));
    $('access-others').appendChild(el('span', 'caption', 'en plus de celle-ci'));
  }

  function renderKeys() {
    var k = data.keys;
    var list = $('keys-list');
    clear(list);
    if (!k) {
      return;
    }
    textOf('keys-count', k.keys.length + ' sur ' + k.max);
    k.keys.forEach(function (key) {
      var li = el('li');
      li.appendChild(ico('key'));
      var txt = el('span', 'key-txt');
      txt.appendChild(el('strong', null, key.label));
      txt.appendChild(el('span', 'caption', 'ajoutée le ' + fmtDate(key.created_at).split(' ')[0] + ' · ' +
        (key.last_used_at ? 'utilisée ' + rel(key.last_used_at) : 'jamais utilisée')));
      li.appendChild(txt);
      if (key.this_session) {
        li.appendChild(badge('Cette session', 'good'));
      }
      list.appendChild(li);
    });
    var dd = $('access-policy');
    dd.textContent = k.policy.space_deletion === 'device-key' ? 'Une clé liée à l’appareil' : 'N’importe laquelle';
    dd.appendChild(el('span', 'caption', 'réglage du serveur'));
  }

  function renderTrash() {
    var t = data.trash;
    var list = $('trash-list');
    clear(list);
    if (!t) {
      return;
    }
    textOf('trash-count', String(t.trash.length));
    $('trash-empty').hidden = t.trash.length > 0;
    list.hidden = t.trash.length === 0;
    t.trash.forEach(function (row) {
      var li = el('li');
      var left = el('span');
      left.appendChild(el('span', 'mono', row.space_id));
      left.appendChild(el('span', 'caption block', 'supprimé ' + rel(row.deleted_at) + ' · ' + fmtBytes(row.size_bytes)));
      li.appendChild(left);
      li.appendChild(el('span', 'muted nowrap', 'effacé ' + rel(row.purge_after)));
      list.appendChild(li);
    });
    var first = t.trash[0] ? t.trash[0].space_id : '<id>';
    textOf('trash-restore-cmd', 'tomi-relay admin trash restore --space ' + first);
    textOf('trash-purge-cmd', 'tomi-relay admin trash purge --space ' + first);
  }

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
      lines.push('Stockage : ' + fmtBytes(s.quota.used_bytes) + ' sur ' + fmtBytes(s.quota.max_bytes) + (p === null ? '' : ' (' + fmtPercent(p) + ')'));
      if (s.default_space_quota) {
        lines.push('Quota par défaut d’un espace : ' + (s.default_space_quota.bytes === null ? 'aucune limite' : fmtBytes(s.default_space_quota.bytes)) +
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
    if (data.trash) {
      var bytes = 0;
      data.trash.trash.forEach(function (r) { bytes += Number(r.size_bytes || 0); });
      lines.push('Corbeille : ' + data.trash.trash.length + ' espace(s), ' + fmtBytes(bytes));
    }
    if (data.keys) {
      var bound = data.keys.keys.filter(function (k) { return k.device_bound; }).length;
      lines.push('Clés d’accès du panneau : ' + data.keys.keys.length + ' (' + bound + ' liée(s) à l’appareil)');
    }
    if (a && a.limits) {
      var l = a.limits;
      lines.push('Limites : total ' + l.max_relay_bytes + ' o ; quota configuré ' + (l.max_space_bytes || 'aucun') +
        ' ; appareils ' + l.max_devices + ' ; élément ' + l.max_envelope_bytes + ' o ; fichier ' + l.max_blob_bytes +
        ' o ; rétention ' + l.event_retention_days + ' j ; inactivité ' + l.inactive_after_days + ' j');
    }
    lines.push('Connexions refusées au panneau (depuis le démarrage) : ' + refusedCount().count);
    lines.push('Navigateur : ' + navigator.userAgent);
    return lines.join('\n');
  }

  // ===== refresh ====================================================================================

  function setError(id, text) {
    var node = $(id);
    node.textContent = text || '';
    node.hidden = !text;
  }

  function guarded(promise, errId, text) {
    return promise.then(
      function () {
        setError(errId, '');
      },
      function (err) {
        if (err && err.message === 'session') {
          throw err;
        }
        setError(errId, text);
      }
    );
  }

  function loadAbout() {
    return read('api/about').then(function (a) {
      data.about = a;
      var t = Date.parse(a.server_time);
      serverOffset = isNaN(t) ? 0 : t - Date.now();
      setSession(a);
      renderAbout();
    });
  }

  function refreshStatusOnly() {
    return Promise.all([
      read('api/status').then(function (s) { data.status = s; }),
      read('api/about').then(function (a) { data.about = a; renderAbout(); renderInvitations(); })
    ]).then(renderStatus, function () {});
  }

  var refreshing = false;

  function refreshAll() {
    if (refreshing) {
      return Promise.resolve();
    }
    refreshing = true;
    $('loading').hidden = false;
    var button = $('refresh');
    button.disabled = true;
    button.textContent = 'Actualisation…';
    // `about` first: the clock offset and the ceilings are used by what follows.
    return loadAbout()
      .then(function () {
        return Promise.all([
          guarded(read('api/status').then(function (s) { data.status = s; }), 'status-error', 'Impossible de lire l’état du relais.'),
          guarded(read('api/spaces').then(function (r) { data.spaces = r.spaces || []; }), 'spaces-error', 'Impossible de lire les espaces.'),
          guarded(read('api/invitations').then(function (r) { data.invitations = r.invitations || []; }), 'invitations-error', 'Impossible de lire les invitations.'),
          guarded(read('api/journal').then(function (r) { data.journal = r; }), 'journal-error', 'Impossible de lire le journal.'),
          guarded(read('api/quota').then(function (r) { data.quota = r.default; }), 'quota-error', 'Impossible de lire le quota par défaut.'),
          guarded(read('api/keys').then(function (r) { data.keys = r; }), 'keys-error', 'Impossible de lire les clés d’accès.'),
          guarded(read('api/trash').then(function (r) { data.trash = r; }), 'trash-error', 'Impossible de lire la corbeille.')
        ]);
      })
      .then(function () {
        renderStatus();
        renderSpaces();
        renderInvitations();
        renderJournal();
        renderQuota();
        renderKeys();
        renderTrash();
        if (selectedSpace && !spaceById(selectedSpace)) {
          window.location.hash = '#espaces';
        } else if (selectedSpace) {
          renderDetail();
          return loadDevices(selectedSpace);
        }
        return null;
      })
      .then(function () {
        var now = new Date();
        var hm = ('0' + now.getHours()).slice(-2) + ':' + ('0' + now.getMinutes()).slice(-2);
        Array.prototype.forEach.call(document.querySelectorAll('.updated-time'), function (n) {
          n.textContent = hm;
        });
      })
      .catch(function (err) {
        if (err && err.message === 'session') {
          lostSession();
          return;
        }
        say('Le relais ne répond pas. Réessaie dans un instant.', 'error');
      })
      .then(function () {
        refreshing = false;
        $('loading').hidden = true;
        button.disabled = false;
        button.textContent = 'Actualiser';
      });
  }

  function openDashboard() {
    return refreshAll().then(function () {
      if (session.idleEnd) {
        showApp();
      } else {
        showAuth('login');
      }
    });
  }

  // ===== navigation: one view at a time, driven by the address ======================================

  var desktop = window.matchMedia ? window.matchMedia('(min-width: 64rem)') : { matches: false };
  var lastView = null;
  var lastSpace = null;

  function route() {
    if (!appShown) {
      return;
    }
    var parts = window.location.hash.replace(/^#/, '').split('/');
    var name = VIEWS[parts[0]] ? parts[0] : 'apercu';
    var space = name === 'espaces' && /^[0-9a-f]{8}$/.test(parts[1] || '') ? parts[1] : null;
    Object.keys(VIEWS).forEach(function (v) {
      $('view-' + v).hidden = v !== name;
    });
    Array.prototype.forEach.call(document.querySelectorAll('.nav a'), function (a) {
      if (a.getAttribute('data-view') === name) {
        a.setAttribute('aria-current', 'page');
      } else {
        a.removeAttribute('aria-current');
      }
    });
    textOf('t-main', VIEWS[name]);
    document.title = (space ? 'Espace ' + space : VIEWS[name]) + ' · Relais Tomi';
    var changed = space !== lastSpace;
    selectedSpace = space;
    lastSpace = space;
    if (space && changed) {
      data.devices = [];
      renderDetail();
      renderDevices();
      loadDevices(space);
    } else {
      renderDetail();
    }
    Array.prototype.forEach.call(document.querySelectorAll('#spaces-body tr'), function (tr) {
      tr.classList.toggle('selected', tr.getAttribute('data-space') === space);
    });
    closeSheet();
    if (name !== lastView || (changed && !desktop.matches)) {
      window.scrollTo(0, 0);
    }
    if (space && changed && desktop.matches) {
      $('h-detail').focus({ preventScroll: true });
    }
    lastView = name;
  }

  // The session sheet (⋮ on a phone), closed by the scrim, Escape, or a change of view.
  function openSheet() {
    $('session-panel').classList.add('open');
    $('scrim').hidden = false;
    $('menu').setAttribute('aria-expanded', 'true');
    $('session-close').focus();
  }

  function closeSheet() {
    var panel = $('session-panel');
    if (!panel.classList.contains('open')) {
      return;
    }
    panel.classList.remove('open');
    $('scrim').hidden = true;
    $('menu').setAttribute('aria-expanded', 'false');
  }

  // "What the relay sees": open on a desktop, folded on a phone.
  function syncBlind() {
    $('blind').open = desktop.matches;
  }

  // ----- desktop keyboard (never for an action that removes, deletes or invites) ----------------

  var gPending = false;
  var gTimer = null;
  var KB = { a: 'apercu', e: 'espaces', i: 'invitations', j: 'journal', r: 'reglages' };

  function typing(target) {
    return target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName);
  }

  function moveSpace(step) {
    var rows = Array.prototype.slice.call(document.querySelectorAll('#spaces-body tr'));
    if (!rows.length) {
      return;
    }
    var at = -1;
    rows.forEach(function (r, i) {
      if (r.getAttribute('data-space') === selectedSpace) {
        at = i;
      }
    });
    var next = rows[Math.max(0, Math.min(rows.length - 1, at + step))];
    window.location.hash = '#espaces/' + next.getAttribute('data-space');
  }

  function onKey(event) {
    if (event.key === 'Escape') {
      if (document.querySelector('dialog[open]')) {
        return;
      }
      if ($('session-panel').classList.contains('open')) {
        closeSheet();
        $('menu').focus();
      } else if (selectedSpace) {
        window.location.hash = '#espaces';
      }
      return;
    }
    if (!appShown || event.ctrlKey || event.metaKey || event.altKey || typing(event.target) || document.querySelector('dialog[open]')) {
      return;
    }
    var k = event.key.toLowerCase();
    if (gPending) {
      gPending = false;
      clearTimeout(gTimer);
      if (KB[k]) {
        event.preventDefault();
        window.location.hash = '#' + KB[k];
      }
      return;
    }
    if (k === 'g') {
      gPending = true;
      gTimer = setTimeout(function () { gPending = false; }, 1500);
    } else if (event.key === '?') {
      $('shortcuts').showModal();
    } else if (k === 'r') {
      refreshAll();
    } else if (k === 'c' && selectedSpace) {
      copyText(selectedSpace, 'Identifiant');
    } else if ((k === 'j' || k === 'arrowdown') && window.location.hash.indexOf('#espaces') === 0) {
      event.preventDefault();
      moveSpace(1);
    } else if ((k === 'k' || k === 'arrowup') && window.location.hash.indexOf('#espaces') === 0) {
      event.preventDefault();
      moveSpace(-1);
    }
  }

  // ===== startup =====================================================================================

  function start() {
    loadTheme();
    Array.prototype.forEach.call(document.getElementsByName('theme'), function (r) {
      r.addEventListener('change', saveTheme);
    });
    $('toast-close').addEventListener('click', function () { say(''); });
    $('enroll-form').addEventListener('submit', enroll);
    $('login-submit').addEventListener('click', login);
    $('enroll-open').addEventListener('click', function () {
      $('enroll-back').hidden = false;
      showAuth('enroll');
    });
    $('enroll-back').addEventListener('click', function () { showAuth('login'); });
    $('logout').addEventListener('click', logout);
    Array.prototype.forEach.call(document.querySelectorAll('[data-close-all]'), function (b) {
      b.addEventListener('click', closeAllSessions);
    });
    $('refresh').addEventListener('click', function () { refreshAll(); });
    $('menu').addEventListener('click', openSheet);
    $('scrim').addEventListener('click', closeSheet);
    $('session-close').addEventListener('click', function () { closeSheet(); $('menu').focus(); });
    $('back').addEventListener('click', function () { window.location.hash = '#espaces'; });
    $('open-blind').addEventListener('click', function () {
      setTimeout(function () {
        $('blind').open = true;
        $('blind').scrollIntoView({ block: 'center' });
      }, 0);
    });
    syncBlind();
    if (desktop.addEventListener) {
      desktop.addEventListener('change', syncBlind);
    }
    window.addEventListener('hashchange', route);
    document.addEventListener('keydown', onKey);
    $('shortcuts-open').addEventListener('click', function () { $('shortcuts').showModal(); });
    $('sc-close').addEventListener('click', function () { $('shortcuts').close(); });

    // The "?" buttons: the explanation stays folded until it is asked for.
    Array.prototype.forEach.call(document.querySelectorAll('.help-btn'), function (b) {
      b.addEventListener('click', function () {
        var open = b.getAttribute('aria-expanded') !== 'true';
        b.setAttribute('aria-expanded', open ? 'true' : 'false');
        $(b.getAttribute('aria-controls')).hidden = !open;
      });
    });
    // Copy buttons, wherever they are drawn.
    document.addEventListener('click', function (event) {
      var b = event.target.closest ? event.target.closest('button[data-copy]') : null;
      if (b) {
        var v = copyValue(b);
        if (v) {
          copyText(v, b.getAttribute('data-copy') === 'Commande' ? 'Commande' : b.getAttribute('data-copy'));
        }
      }
    });

    // Spaces: sorting, a click on the row.
    Array.prototype.forEach.call(document.querySelectorAll('#spaces-table button.sort'), function (b) {
      b.addEventListener('click', function () { setSort('spaces', b.getAttribute('data-sort')); });
    });
    $('spaces-sort').addEventListener('change', function (e) {
      sorts.spaces.key = e.target.value;
      sorts.spaces.dir = e.target.value === 'id' ? 1 : -1;
      renderSpaces();
    });
    $('devices-sort').addEventListener('change', function (e) {
      sorts.devices.key = e.target.value;
      sorts.devices.dir = e.target.value === 'name' || e.target.value === 'platform' ? 1 : -1;
      renderDevices();
    });
    $('spaces-body').addEventListener('click', function (event) {
      var tr = event.target.closest('tr');
      if (tr && !event.target.closest('a, button')) {
        window.location.hash = '#espaces/' + tr.getAttribute('data-space');
      }
    });
    $('spaces-retry').addEventListener('click', function () { refreshAll(); });
    $('banner-close').addEventListener('click', function () { $('spaces-banner').hidden = true; });
    $('spaces-banner').setAttribute('tabindex', '-1');

    // The detail: the quota of the space, the devices, the deletion.
    $('sq-edit').addEventListener('click', function () {
      var s = spaceById(selectedSpace);
      $('sq-form').hidden = false;
      $('sq-edit').hidden = true;
      $('sq-edit').setAttribute('aria-expanded', 'true');
      $('sq-mib').value = s && s.max_bytes ? String(Math.round(s.max_bytes / 1048576 * 10) / 10).replace('.', ',') : '';
      previewSpaceQuota();
      $('sq-mib').focus();
    });
    $('sq-mib').addEventListener('input', previewSpaceQuota);
    $('sq-form').addEventListener('submit', function (event) {
      event.preventDefault();
      var bytes = previewSpaceQuota();
      if (bytes === null) {
        $('sq-mib').focus();
        return;
      }
      saveSpaceQuota(bytes, 'Quota de l’espace réglé : ' + fmtBytes(bytes) + '.');
    });
    $('sq-default').addEventListener('click', function () {
      saveSpaceQuota(null, 'Quota de l’espace : retour au quota par défaut.');
    });
    $('sq-none').addEventListener('click', function () {
      saveSpaceQuota(0, 'Quota de l’espace : aucune limite.');
    });
    $('d-devices').addEventListener('change', function (event) {
      if (event.target.classList.contains('dev-sel')) {
        syncSelection();
      }
    });
    $('d-devices').addEventListener('click', function (event) {
      var button = event.target.closest('.dev-revoke');
      if (button) {
        askRevoke([button.closest('.dev').getAttribute('data-device')], button);
      }
    });
    $('d-selclear').addEventListener('click', function () {
      Array.prototype.forEach.call(document.querySelectorAll('#d-devices .dev-sel'), function (b) { b.checked = false; });
      syncSelection();
    });
    $('d-selrevoke').addEventListener('click', function () { askRevoke(selectedIds(), $('d-selrevoke')); });
    $('d-delete').addEventListener('click', function () { askDelete($('d-delete')); });

    // Invitations.
    $('invite-new').addEventListener('click', function () { askInvite('new', $('invite-new')); });
    $('invite-rotate').addEventListener('click', function () { askInvite('rotate', $('invite-rotate')); });
    $('invite-cancel').addEventListener('click', cancelInvitations);
    $('token-copy').addEventListener('click', function () { copyText($('token').textContent, 'Jeton'); });
    $('token-link-copy').addEventListener('click', function () { copyText($('token-link').textContent, 'Lien'); });
    $('token-hide').addEventListener('click', hideToken);
    // Leaving the page clears the displayed token (and its QR).
    window.addEventListener('pagehide', hideToken);

    // Settings.
    $('quota-form').addEventListener('submit', submitQuota);
    $('quota-mib').addEventListener('input', previewQuota);
    $('quota-unlimited').addEventListener('click', quotaUnlimited);
    $('quota-reset').addEventListener('click', function () {
      saveDefaultQuota(null, 'Quota par défaut rétabli à celui de la configuration du relais.');
    });
    $('diagnostic-copy').addEventListener('click', function () {
      var text = diagnosticText();
      $('diagnostic').textContent = text;
      copyText(text, 'Diagnostic');
    });

    // The sheet of the step-up.
    $('su-input').addEventListener('input', syncTyped);
    $('su-cancel').addEventListener('click', closeStep);
    $('su-ok').addEventListener('click', onStepOk);

    if (!webauthnAvailable()) {
      showAuth('nojs');
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
        $('enroll-back').hidden = true;
        showAuth(r.data.enrolled ? 'login' : 'enroll');
        return null;
      })
      .catch(function () {
        say('Le relais ne répond pas. Recharge la page.', 'error');
        showAuth('login');
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
