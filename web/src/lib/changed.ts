// A Svelte action for one-off change cues (plan §5 rows 5–6): when the watched value
// differs from the last one, add a class for one animation, then take it off. The first
// value (initial load) never animates; each row or number keeps its own last value.

export function changed(node: HTMLElement, opts: { value: unknown; cls?: string }) {
  let last = JSON.stringify(opts.value)
  let cls = opts.cls ?? 'flash'
  // With Motion off no animation runs, so no animationend comes: a timer takes the class
  // off too, or turning Motion back on would flash every row that once changed.
  let timer: ReturnType<typeof setTimeout> | undefined
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
        const on = cls
        clearTimeout(timer)
        timer = setTimeout(() => node.classList.remove(on), 2000)
      }
      last = v
    },
    destroy() {
      clearTimeout(timer)
      node.removeEventListener('animationend', done)
    },
  }
}
