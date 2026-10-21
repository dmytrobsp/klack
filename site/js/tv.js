// The huddle capture's shared screen (LP7b): its Swan Lake loop and grain run only while it is on screen.
// Off screen the .tv gets `paused` (site.css pauses its animations); reduced motion has no animations at all.
// No network, no third-party code. Tested in tools/site/tv.test.mjs.

/** Watches every `.capture .tv` in `doc`; returns the observer (null without screens or IntersectionObserver). */
export function pauseOffScreen(doc = document, IO = doc.defaultView && doc.defaultView.IntersectionObserver) {
  const tvs = [...doc.querySelectorAll('.capture .tv')];
  if (!tvs.length || typeof IO !== 'function') return null;
  const io = new IO(entries => {
    for (const e of entries) e.target.classList.toggle('paused', !e.isIntersecting);
  });
  for (const tv of tvs) {
    tv.classList.add('paused');  // until the observer says it is in view
    io.observe(tv);
  }
  return io;
}
