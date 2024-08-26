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

ws.onopen = async (_) => {
  const attachAddon = new AttachAddon(ws)
  term.loadAddon(attachAddon)

  const response = await fetch(window.location + "/active_connections")
  const json = await response.json()

  console.log(json)
  const active_connections = json["active_connections"]

  if (active_connections > 1) {
    if (active_connections === 2) {
      appendWsAlert("There is 1 other active connection to this device")
    } else {
      appendWsAlert(`There are ${active_connections - 1} other active connections to this device`)
    }
  }
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
