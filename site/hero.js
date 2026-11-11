// Starts the hero's shimmer. Without this script, or with reduced motion, the scene stays still.
(() => {
  const art = document.querySelector(".ascii");
  if (!art || matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  // Animate only while the scene is on screen.
  new IntersectionObserver(([entry]) => art.classList.toggle("live", entry.isIntersecting)).observe(art);
})();
