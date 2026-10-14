import { mount } from 'svelte'
import '../app.css'
import Card from './Card.svelte'
import { PostMessageHost, applyTheme } from './host'

// Dark until the host says otherwise (never prefers-color-scheme in the sandbox).
applyTheme(undefined)
const card = mount(Card, {
  target: document.getElementById('card')!,
  props: { host: new PostMessageHost() },
})

export default card
