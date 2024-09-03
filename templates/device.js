import {Terminal} from "@xterm/xterm"
import {AttachAddon} from "@xterm/addon-attach";
import {FitAddon} from "@xterm/addon-fit/src/FitAddon";

import "@xterm/xterm/css/xterm.css"

const checkAutoResize = document.getElementById("checkAutoResize")
const terminalSizeFields = document.getElementById("terminalSizeFields")
const numTerminalColumns = document.getElementById("numTerminalColumns")
const numTerminalRows = document.getElementById("numTerminalRows")

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
  const [cols, rows] = getClampedProposedDims()

  console.log(`auto-resizing terminal to ${cols} by ${rows}`)
  term.resize(cols, rows)

  numTerminalColumns.value = cols
  numTerminalRows.value = rows
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

const terminalElement = document.getElementById("terminal")
term.open(terminalElement)
term.loadAddon(fitAddon)

numTerminalColumns.value = term.options.cols
numTerminalRows.value = term.options.rows

document.getElementById("btnApplyTerminalSize").onclick = (ev) => {
  const cols = document.getElementById("numTerminalColumns").value
  const rows = document.getElementById("numTerminalRows").value

  console.log("resizing terminal to " + cols + " by " + rows)
  term.resize(cols, rows)
}

checkAutoResize.oninput = (ev) => {
  if (ev.target.checked) {
    terminalElement.classList.add("flex-grow-1")

    resizeTerminalToFit()
    terminalSizeFields.setAttribute("disabled", "")
  } else {
    terminalElement.classList.remove("flex-grow-1")

    term.resize(term.options.cols, term.options.rows)

    terminalSizeFields.removeAttribute("disabled")
    numTerminalColumns.value = term.options.cols
    numTerminalRows.value = term.options.rows
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

const btnPopout = document.getElementById("btnPopout")
if (btnPopout != null) {
  btnPopout.onclick = (ev) => {
    window.open(window.location.pathname + "?p=true", "Serri", "menubar=no,toolbar=no,location=no")
  }
}

// TODO: debounce
const resizeObserver = new ResizeObserver(entries => {
  for (let entry of entries) {
    if (checkAutoResize.checked) {
      resizeTerminalToFit()
    }
  }
})

resizeObserver.observe(terminalElement)
