// ============================================================
// skill_pool site — minimal vanilla JS
// Handles: search modal (⌘K), media tabs, mermaid init, TOC highlight
// ============================================================

(function () {
  'use strict';

  // ---------- Mermaid init ----------
  if (window.mermaid) {
    window.mermaid.initialize({
      startOnLoad: true,
      theme: 'base',
      themeVariables: {
        background: '#050805',
        primaryColor: '#0a1a0a',
        primaryTextColor: '#d5ffd5',
        primaryBorderColor: '#00ff88',
        lineColor: '#2a8a2a',
        secondaryColor: '#0a1a0a',
        tertiaryColor: '#0a1a0a',
        fontFamily: 'JetBrains Mono, monospace',
        edgeLabelBackground: '#050805',
        clusterBkg: '#020602',
        clusterBorder: '#144a14',
        nodeBorder: '#00b860',
        textColor: '#d5ffd5',
      },
      flowchart: { curve: 'basis' },
    });
  }

  // ---------- Media tabs ----------
  document.querySelectorAll('[data-tabs]').forEach(function (group) {
    var tabs = group.querySelectorAll('.media-tab');
    var panes = group.querySelectorAll('.media-pane');
    tabs.forEach(function (tab, i) {
      tab.addEventListener('click', function () {
        tabs.forEach(function (t) { t.classList.remove('active'); });
        panes.forEach(function (p) { p.classList.remove('active'); });
        tab.classList.add('active');
        if (panes[i]) panes[i].classList.add('active');
        // Pause all videos in this group, then play the active one (if it's a video).
        panes.forEach(function (p) {
          p.querySelectorAll('video').forEach(function (v) { v.pause(); });
        });
        if (panes[i]) {
          var v = panes[i].querySelector('video');
          if (v) v.play().catch(function () { /* autoplay blocked is fine */ });
        }
      });
    });
  });

  // ---------- TOC active section highlight ----------
  var tocLinks = document.querySelectorAll('.toc a[href^="#"]');
  if (tocLinks.length) {
    var sections = Array.from(tocLinks).map(function (a) {
      var id = a.getAttribute('href').slice(1);
      return { id: id, el: document.getElementById(id), link: a };
    }).filter(function (s) { return s.el; });
    var onScroll = function () {
      var y = window.scrollY + 120;
      var active = sections[0];
      sections.forEach(function (s) {
        if (s.el.offsetTop <= y) active = s;
      });
      sections.forEach(function (s) {
        s.link.classList.toggle('active', s === active);
      });
    };
    window.addEventListener('scroll', onScroll, { passive: true });
    onScroll();
  }

  // ---------- Search modal ----------
  var modal       = document.getElementById('search-modal');
  var openBtn     = document.querySelector('.search-btn');
  var input       = document.getElementById('search-input');
  var results     = document.getElementById('search-results');
  var searchIndex = null;
  var docs        = [];
  var selectedIdx = 0;

  function loadIndex() {
    if (searchIndex || !window.lunr) return Promise.resolve();
    return fetch('search-index.json')
      .then(function (r) { return r.json(); })
      .then(function (data) {
        docs = data.docs;
        searchIndex = window.lunr(function () {
          this.ref('id');
          this.field('title', { boost: 10 });
          this.field('section', { boost: 4 });
          this.field('body');
          var self = this;
          data.docs.forEach(function (d) { self.add(d); });
        });
      })
      .catch(function (e) {
        console.warn('search index failed', e);
      });
  }

  function openModal() {
    if (!modal) return;
    modal.classList.add('open');
    loadIndex().then(function () {
      input.focus();
      input.value = '';
      results.innerHTML = '';
      renderEmpty();
    });
  }

  function closeModal() {
    if (modal) modal.classList.remove('open');
  }

  function renderEmpty() {
    results.innerHTML = '<div class="search-empty">type to search — esc to close · ↑↓ navigate · enter to open</div>';
  }

  function render(query) {
    if (!query.trim()) { renderEmpty(); return; }
    if (!searchIndex) { results.innerHTML = '<div class="search-empty">index loading…</div>'; return; }
    var hits;
    try {
      // wildcard each non-stop term for forgiving prefix match
      var q = query.trim().split(/\s+/).map(function (t) { return t + '*'; }).join(' ');
      hits = searchIndex.search(q);
    } catch (e) {
      hits = [];
    }
    if (!hits.length) {
      results.innerHTML = '<div class="search-empty">no matches for &laquo;' + escape(query) + '&raquo;</div>';
      return;
    }
    selectedIdx = 0;
    var html = hits.slice(0, 12).map(function (h, i) {
      var d = docs.find(function (x) { return x.id === h.ref; });
      if (!d) return '';
      var snippet = (d.body || '').slice(0, 140);
      return '<a class="search-result' + (i === 0 ? ' selected' : '') + '" href="' + d.url + '" data-idx="' + i + '">' +
        '<div class="search-result-section">' + escape(d.section || '') + '</div>' +
        '<div class="search-result-title">' + escape(d.title) + '</div>' +
        '<div class="search-result-context">' + escape(snippet) + '…</div>' +
        '</a>';
    }).join('');
    results.innerHTML = html;
  }

  function escape(s) {
    return String(s).replace(/[&<>"']/g, function (c) {
      return ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]);
    });
  }

  function moveSelection(dir) {
    var items = results.querySelectorAll('.search-result');
    if (!items.length) return;
    items[selectedIdx] && items[selectedIdx].classList.remove('selected');
    selectedIdx = (selectedIdx + dir + items.length) % items.length;
    items[selectedIdx].classList.add('selected');
    items[selectedIdx].scrollIntoView({ block: 'nearest' });
  }

  if (openBtn) openBtn.addEventListener('click', openModal);
  if (input) input.addEventListener('input', function () { render(input.value); });

  document.addEventListener('keydown', function (e) {
    var meta = e.metaKey || e.ctrlKey;
    if (meta && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      openModal();
      return;
    }
    if (!modal || !modal.classList.contains('open')) return;
    if (e.key === 'Escape') { closeModal(); return; }
    if (e.key === 'ArrowDown') { e.preventDefault(); moveSelection(1); return; }
    if (e.key === 'ArrowUp') { e.preventDefault(); moveSelection(-1); return; }
    if (e.key === 'Enter') {
      var sel = results.querySelector('.search-result.selected');
      if (sel) window.location = sel.getAttribute('href');
    }
  });

  if (modal) {
    modal.addEventListener('click', function (e) { if (e.target === modal) closeModal(); });
  }

  // ---------- Year stamp ----------
  var y = document.getElementById('year');
  if (y) y.textContent = String(new Date().getFullYear());

  // ---------- Brand cursor blink kicker (re-enables animation after nav) ----------
  document.querySelectorAll('.brand .cursor').forEach(function (c) {
    c.style.animation = 'none';
    void c.offsetWidth;
    c.style.animation = '';
  });
})();
