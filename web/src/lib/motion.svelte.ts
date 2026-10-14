// The Motion setting (plan §5): one switch for every looping animation, the WCAG 2.2.2
// pause control. It sets `data-motion` on <html> (the CSS gates loops on it) and is
// remembered in localStorage. Default on, unless the system asks for reduced motion.

export const MOTION_KEY = 'bridge-screen/motion'

function reducedMotion(): boolean {
  try {
    return window.matchMedia('(prefers-reduced-motion: reduce)').matches
  } catch {
    return false
  }
}

function stored(): 'on' | 'off' | null {
  try {
    const v = window.localStorage.getItem(MOTION_KEY)
    return v === 'on' || v === 'off' ? v : null
  } catch {
    return null
  }
}

class Motion {
  on = $state(true)
  /** Wall-clock time for live timers: ticks each second, or each minute with motion off. */
  now = $state(Date.now())
  #timer: ReturnType<typeof setInterval> | null = null

  constructor() {
    if (typeof window === 'undefined') return
    const s = stored()
    this.on = s ? s === 'on' : !reducedMotion()
    this.#apply()
  }

  #apply() {
    if (typeof document !== 'undefined')
      document.documentElement.dataset.motion = this.on ? 'on' : 'off'
    this.#restart()
  }

  #restart() {
    if (this.#timer) clearInterval(this.#timer)
    if (typeof window === 'undefined') return
    this.now = Date.now()
    this.#timer = setInterval(() => (this.now = Date.now()), this.on ? 1000 : 60_000)
  }

  set(on: boolean) {
    this.on = on
    try {
      window.localStorage.setItem(MOTION_KEY, on ? 'on' : 'off')
    } catch {
      // Private mode or blocked storage: the setting lasts for this page only.
    }
    this.#apply()
  }

  toggle() {
    this.set(!this.on)
  }
}

export const motion = new Motion()
