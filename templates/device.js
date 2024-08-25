import {Terminal} from "@xterm/xterm"
import {AttachAddon} from "@xterm/addon-attach";

import "@xterm/xterm/css/xterm.css"

const term = new Terminal({
  fontFamily: "monospace"
})

const ws = new WebSocket(window.location + "/ws")

ws.onerror = (ev) => {
  console.log("ws error: " + ev.message)
  console.log(ev)
}

ws.onopen = (_) => {
  const attachAddon = new AttachAddon(ws)
  term.loadAddon(attachAddon)
}

ws.onclose = (ev) => {
  console.log("ws closing: " + ev.reason)
  console.log(ev)

  if (!ev.wasClean) {
    if (ev.reason.length > 0) {
      appendWsAlert(ev.reason)
    } else {
      appendWsAlert("Connection closed unexpectedly")
    }
  }
}

term.open(document.getElementById("terminal"))

document.getElementById("numTerminalColumns").value = term.options.cols
document.getElementById("numTerminalRows").value = term.options.rows

document.getElementById("btnApplyTerminalSize").onclick = (ev) => {
  const cols = document.getElementById("numTerminalColumns").value
  const rows = document.getElementById("numTerminalRows").value

  console.log("resizing terminal to " + cols + " by " + rows)
  term.resize(cols, rows)
}

document.getElementById("btnClearHistory").onclick = async (ev) => {
  console.log("clearing history")
  term.clear()

  const response = await fetch(window.location + "/clear_history", {
    method: "POST"
  })

  console.log(response.status)
}

document.getElementById("checkPreserveHistory").oninput = async (ev) => {
  console.log(ev.target.checked)

  const response = await fetch(window.location + "/preserve_history", {
    method: "POST",
    body: JSON.stringify({preserve_history: ev.target.checked}),
    headers: {
      "Content-Type": "application/json"
    }
  })

  console.log(response.status)
}
