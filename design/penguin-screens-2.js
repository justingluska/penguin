/* Presentation helpers only: theme-aware navigation and the inbox backdrop.
   These static screens do not read credentials, connect accounts or save settings. */
(function () {
  function reflectTheme() {
    var theme = document.documentElement.dataset.theme;
    document.querySelectorAll('a[data-theme-link]').forEach(function (link) {
      var url = new URL(link.getAttribute('href'), location.href);
      url.searchParams.set('theme', theme);
      link.href = url.href;
    });
    document.querySelectorAll('[data-theme-choice]').forEach(function (button) {
      var active = button.dataset.themeChoice === theme;
      button.classList.toggle('selected', active);
      button.setAttribute('aria-pressed', String(active));
    });
    var bg = document.getElementById('shortcuts-bg');
    if (bg) bg.src = '01-inbox.html?theme=' + theme;
  }
  document.querySelectorAll('[data-theme-choice]').forEach(function (button) {
    button.addEventListener('click', function () {
      document.documentElement.dataset.theme = button.dataset.themeChoice;
    });
  });
  reflectTheme();
  new MutationObserver(reflectTheme).observe(document.documentElement, {
    attributes: true, attributeFilter: ['data-theme']
  });
  document.addEventListener('keydown', function (event) {
    if (event.key === 'Escape' && document.getElementById('shortcut-close')) {
      document.getElementById('shortcut-close').click();
    }
  });
})();
