import {Terminal} from '@xterm/xterm'
import {AttachAddon} from '@xterm/addon-attach';

import "@xterm/xterm/css/xterm.css"

const term = new Terminal({
  fontFamily: 'monospace'
})

term.open(document.getElementById('terminal'))

document.getElementById('numTerminalColumns').value = term.options.cols
document.getElementById('numTerminalRows').value = term.options.rows

document.getElementById('btnApplyTerminalSize').onclick = (ev) => {
  const cols = document.getElementById('numTerminalColumns').value
  const rows = document.getElementById('numTerminalRows').value

  console.log('resizing terminal to ' + cols + ' by ' + rows)
  term.resize(cols, rows)
}

const ws = new WebSocket(window.location + '/ws')

ws.onerror = (ev) => {
  console.log("ws error:")
  console.log(ev)
}

ws.onopen = (ev) => {
  const attachAddon = new AttachAddon(ws)
  term.loadAddon(attachAddon)
}

ws.onclose = (ev) => {
  console.log("ws closing:")
  console.log(ev)

  if (!ev.wasClean) {
    appendWsAlert("WebSocket connection closed")
  }
}
