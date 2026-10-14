// A Svelte action for one-off change cues (plan §5 rows 5–6): when the watched value
// differs from the last one, add a class for one animation, then take it off. The first
// value (initial load) never animates; each row or number keeps its own last value.

export function changed(node: HTMLElement, opts: { value: unknown; cls?: string }) {
  let last = JSON.stringify(opts.value)
  let cls = opts.cls ?? 'flash'
  const done = (e: AnimationEvent) => {
    if (e.target === node) node.classList.remove(cls)
  }
  node.addEventListener('animationend', done)
  return {
    update(next: { value: unknown; cls?: string }) {
      const v = JSON.stringify(next.value)
      cls = next.cls ?? 'flash'
      if (v !== last) {
        node.classList.remove(cls)
        // Restart the animation if it is still running.
        void node.offsetWidth
        node.classList.add(cls)
      }
      last = v
    },
    destroy() {
      node.removeEventListener('animationend', done)
    },
  }
}
