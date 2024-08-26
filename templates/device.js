import {Terminal} from "@xterm/xterm"
import {AttachAddon} from "@xterm/addon-attach";
import {FitAddon} from "@xterm/addon-fit/src/FitAddon";

import "@xterm/xterm/css/xterm.css"

const term = new Terminal({
  fontFamily: "monospace"
})

const fitAddon = new FitAddon()

const getClampedProposedDims = () => {
  const proposedDims = fitAddon.proposeDimensions()
  const cols = proposedDims.cols > 80 ? proposedDims.cols : 80
  const rows = proposedDims.rows > 24 ? proposedDims.rows : 24

  return [cols, rows]
}

const resizeTerminalToFit = () => {
  const proposedDims = fitAddon.proposeDimensions()
  const cols = proposedDims.cols > 80 ? proposedDims.cols : 80
  const rows = proposedDims.rows > 24 ? proposedDims.rows : 24

  console.log(`auto-resizing terminal to ${cols} by ${rows}`)
  term.resize(cols, rows)
}

const ws = new WebSocket(window.location.pathname + "/ws")

ws.onerror = (ev) => {
  console.log("ws error: " + ev.message)
  console.log(ev)
}

ws.onopen = async (_) => {
  const attachAddon = new AttachAddon(ws)
  term.loadAddon(attachAddon)

  const response = await fetch(window.location.pathname + "/active_connections")
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
term.loadAddon(fitAddon)

document.getElementById("numTerminalColumns").value = term.options.cols
document.getElementById("numTerminalRows").value = term.options.rows

document.getElementById("btnApplyTerminalSize").onclick = (ev) => {
  const cols = document.getElementById("numTerminalColumns").value
  const rows = document.getElementById("numTerminalRows").value

  console.log("resizing terminal to " + cols + " by " + rows)
  term.resize(cols, rows)
}

document.getElementById("checkAutoResize").oninput = (ev) => {
  if (ev.target.checked) {
    document.getElementById("terminal").classList.add("flex-grow-1")

    resizeTerminalToFit()
    const [cols, rows] = getClampedProposedDims()

    document.getElementById("terminalSizeFields").setAttribute("disabled", "")
    document.getElementById("numTerminalColumns").value = cols
    document.getElementById("numTerminalRows").value = rows
  } else {
    document.getElementById("terminal").classList.remove("flex-grow-1")

    term.resize(term.options.cols, term.options.rows)

    document.getElementById("terminalSizeFields").removeAttribute("disabled")
    document.getElementById("numTerminalColumns").value = term.options.cols
    document.getElementById("numTerminalRows").value = term.options.rows
  }
}

document.getElementById("btnClearHistory").onclick = async (ev) => {
  console.log("clearing history")
  term.clear()

  const response = await fetch(window.location.pathname + "/clear_history", {
    method: "POST"
  })

  console.log(response.status)
}

document.getElementById("checkPreserveHistory").oninput = async (ev) => {
  console.log(ev.target.checked)

  const response = await fetch(window.location.pathname + "/preserve_history", {
    method: "POST",
    body: JSON.stringify({preserve_history: ev.target.checked}),
    headers: {
      "Content-Type": "application/json"
    }
  })

  console.log(response.status)
}

// TODO: debounce
window.onresize = (ev) => {
  if (document.getElementById("checkAutoResize").checked) {
    resizeTerminalToFit()
  }
}
