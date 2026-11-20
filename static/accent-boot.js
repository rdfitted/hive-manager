(() => {
  let accent = 'honey';
  try {
    if (localStorage.getItem('hive-manager-accent') === 'sky') accent = 'sky';
  } catch {
    // The canvas and the default accent remain available without storage.
  }
  document.documentElement.dataset.accent = accent;
})();
