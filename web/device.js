import {Terminal} from '@xterm/xterm'
import { AttachAddon } from '@xterm/addon-attach';

import './device.scss'

const ws = new WebSocket(window.location + '/ws')

const term = new Terminal()
const attachAddon = new AttachAddon(ws);
term.loadAddon(attachAddon);
term.open(document.getElementById('terminal'))
