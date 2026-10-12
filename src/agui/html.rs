//! Embedded Visual Graph Editor & AG-UI Runtime Inspector.
//! Serves an interactive visual canvas that connects directly to the AG-UI event stream.

pub const VISUAL_EDITOR_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>term-ed | AG-UI Agent Routing Visualizer & Decision Canvas</title>
  <style>
    :root {
      --bg: #1e1e1e;
      --panel: #252526;
      --panel-border: #3c3c3c;
      --text: #cccccc;
      --text-bright: #ffffff;
      --text-muted: #858585;
      --accent: #0e639c;
      --accent-bright: #007acc;
      --green: #4ec9b0;
      --green-dark: #2ea043;
      --orange: #ce9178;
      --yellow: #dcdcaa;
      --red: #f44747;
      --purple: #c586c0;
      --cyan: #9cdcfe;
    }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif;
      background: var(--bg);
      color: var(--text);
      display: flex;
      flex-direction: column;
      height: 100vh;
      overflow: hidden;
    }
    header {
      height: 48px;
      background: #181818;
      border-bottom: 1px solid var(--panel-border);
      display: flex;
      align-items: center;
      justify-content: space-between;
      padding: 0 16px;
    }
    .logo {
      display: flex;
      align-items: center;
      gap: 10px;
      font-weight: 600;
      font-size: 14px;
      color: var(--text-bright);
    }
    .badge {
      background: var(--accent);
      color: white;
      font-size: 11px;
      padding: 2px 8px;
      border-radius: 12px;
      font-weight: 500;
    }
    .status-tag {
      font-size: 12px;
      display: flex;
      align-items: center;
      gap: 6px;
    }
    .status-dot {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: #73c991;
      box-shadow: 0 0 8px #73c991;
    }
    .status-dot.disconnected {
      background: var(--red);
      box-shadow: 0 0 8px var(--red);
    }
    .main-layout {
      flex: 1;
      display: grid;
      grid-template-columns: 1fr 380px;
      height: calc(100vh - 48px);
      position: relative;
    }
    .canvas-container {
      position: relative;
      background: radial-gradient(#2d2d30 1px, transparent 1px);
      background-size: 24px 24px;
      overflow: hidden;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    svg#graph-canvas {
      width: 100%;
      height: 100%;
      position: absolute;
      top: 0;
      left: 0;
    }
    .sidebar {
      background: var(--panel);
      border-left: 1px solid var(--panel-border);
      display: flex;
      flex-direction: column;
      height: 100%;
    }
    .tabs {
      display: flex;
      border-bottom: 1px solid var(--panel-border);
      background: #1f1f20;
    }
    .tab {
      padding: 10px 16px;
      font-size: 12px;
      cursor: pointer;
      color: var(--text-muted);
      border-bottom: 2px solid transparent;
      user-select: none;
    }
    .tab.active {
      color: var(--text-bright);
      border-bottom-color: var(--accent-bright);
      background: var(--panel);
    }
    .tab-content {
      flex: 1;
      overflow-y: auto;
      padding: 14px;
      display: none;
      font-size: 13px;
    }
    .tab-content.active { display: block; }

    /* Node Styles */
    .node-card {
      position: absolute;
      width: 220px;
      background: #252526;
      border: 1px solid #454545;
      border-radius: 8px;
      box-shadow: 0 4px 16px rgba(0,0,0,0.5);
      cursor: pointer;
      transition: all 0.2s ease;
      user-select: none;
      z-index: 10;
    }
    .node-card:hover {
      border-color: var(--accent-bright);
      transform: translateY(-2px);
      box-shadow: 0 6px 20px rgba(0,122,204,0.3);
    }
    .node-card.active-pulse {
      border-color: var(--green);
      box-shadow: 0 0 16px var(--green);
      animation: pulse 1.5s infinite;
    }
    @keyframes pulse {
      0% { box-shadow: 0 0 4px var(--green); }
      50% { box-shadow: 0 0 18px var(--green); }
      100% { box-shadow: 0 0 4px var(--green); }
    }
    .node-header {
      padding: 8px 12px;
      border-bottom: 1px solid #333333;
      display: flex;
      align-items: center;
      justify-content: space-between;
      border-top-left-radius: 7px;
      border-top-right-radius: 7px;
      font-size: 11px;
      text-transform: uppercase;
      font-weight: 700;
      letter-spacing: 0.5px;
    }
    .node-agent .node-header { background: #1a334d; color: var(--cyan); }
    .node-gate .node-header { background: #3d2c18; color: var(--yellow); }
    .node-tool .node-header { background: #1e3328; color: var(--green); }
    .node-reviewer .node-header { background: #351d3b; color: var(--purple); }
    .node-body {
      padding: 10px 12px;
    }
    .node-title {
      font-size: 13px;
      font-weight: 600;
      color: var(--text-bright);
      margin-bottom: 4px;
    }
    .node-sub {
      font-size: 11px;
      color: var(--text-muted);
    }

    /* Edge line */
    .edge-line {
      stroke: #555555;
      stroke-width: 2;
      stroke-dasharray: 4, 4;
      transition: stroke 0.3s;
    }
    .edge-line.active {
      stroke: var(--accent-bright);
      stroke-width: 3;
      stroke-dasharray: none;
      animation: flow 1s linear infinite;
    }

    /* Event stream entries */
    .event-entry {
      background: #1e1e1e;
      border: 1px solid #333333;
      border-radius: 6px;
      padding: 8px 10px;
      margin-bottom: 8px;
      font-size: 11px;
      font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
      animation: fadeIn 0.3s ease;
    }
    @keyframes fadeIn {
      from { opacity: 0; transform: translateY(-4px); }
      to { opacity: 1; transform: translateY(0); }
    }
    .event-type {
      font-weight: 600;
      color: var(--cyan);
      margin-bottom: 4px;
    }
    .event-type.decision { color: var(--yellow); }
    .event-type.patch { color: var(--green); }
    .event-type.tool { color: var(--orange); }

    /* Diff View */
    .diff-box {
      font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
      background: #181818;
      border-radius: 4px;
      padding: 8px;
      margin-top: 6px;
      font-size: 11px;
      overflow-x: auto;
    }
    .diff-del { color: #f85149; background: rgba(248,81,73,0.15); display: block; }
    .diff-add { color: #56d364; background: rgba(86,211,100,0.15); display: block; }

    /* Controls bar */
    .controls-overlay {
      position: absolute;
      bottom: 20px;
      left: 20px;
      background: rgba(30, 30, 30, 0.95);
      border: 1px solid var(--panel-border);
      border-radius: 8px;
      padding: 10px 14px;
      display: flex;
      gap: 10px;
      align-items: center;
      z-index: 50;
      box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    }
    .btn {
      background: var(--accent);
      color: white;
      border: none;
      padding: 6px 12px;
      border-radius: 4px;
      font-size: 12px;
      cursor: pointer;
      font-weight: 500;
      transition: background 0.15s;
    }
    .btn:hover { background: var(--accent-bright); }
    .btn-secondary { background: #3c3c3c; }
    .btn-secondary:hover { background: #505050; }
    .btn-danger { background: #a32424; }
    .btn-danger:hover { background: #cb3535; }
  </style>
</head>
<body>
  <header>
    <div class="logo">
      <span>term-ed</span>
      <span class="badge">AG-UI Runtime</span>
      <span style="color: var(--text-muted); font-size: 12px; font-weight: normal;">MCP • A2A • AEP • Jev Decision Engine</span>
    </div>
    <div class="status-tag">
      <div id="conn-dot" class="status-dot"></div>
      <span id="conn-status">Listening to AG-UI Stream (/events)</span>
    </div>
  </header>

  <div class="main-layout">
    <div class="canvas-container">
      <svg id="graph-canvas">
        <defs>
          <marker id="arrow" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#777777"/>
          </marker>
        </defs>
      </svg>

      <!-- Graph Nodes -->
      <div id="node_orch" class="node-card node-agent" style="top: 100px; left: 60px;">
        <div class="node-header"><span>Agent (A2A)</span><span>Planner</span></div>
        <div class="node-body">
          <div class="node-title">Architect Agent</div>
          <div class="node-sub">Protocol: A2A Task Orchestrator</div>
        </div>
      </div>

      <div id="node_coder" class="node-card node-agent" style="top: 100px; left: 340px;">
        <div class="node-header"><span>Agent</span><span>termed AI</span></div>
        <div class="node-body">
          <div class="node-title">Coder Agent</div>
          <div class="node-sub">In-process Autonomous Coder</div>
        </div>
      </div>

      <div id="node_gate" class="node-card node-gate" style="top: 260px; left: 340px;">
        <div class="node-header"><span>Decision Gate</span><span>Jev (TypeSafe)</span></div>
        <div class="node-body">
          <div class="node-title">Jev Risk Gate</div>
          <div class="node-sub">ask_score (0.0 - 1.0) Tiering</div>
        </div>
      </div>

      <div id="node_mcp_fs" class="node-card node-tool" style="top: 70px; left: 640px;">
        <div class="node-header"><span>MCP Tools</span><span>File Ops</span></div>
        <div class="node-body">
          <div class="node-title">Workspace MCP</div>
          <div class="node-sub">read_file, edit_file, search</div>
        </div>
      </div>

      <div id="node_mcp_sh" class="node-card node-tool" style="top: 260px; left: 640px;">
        <div class="node-header"><span>MCP Runner</span><span>Shell</span></div>
        <div class="node-body">
          <div class="node-title">Shell MCP Runner</div>
          <div class="node-sub">run_command with auto-gate</div>
        </div>
      </div>

      <div id="node_judge" class="node-card node-reviewer" style="top: 420px; left: 340px;">
        <div class="node-header"><span>Judge (Jev)</span><span>ask_noul</span></div>
        <div class="node-body">
          <div class="node-title">Jev-Judge Reviewer</div>
          <div class="node-sub">Post-edit Diff Verification</div>
        </div>
      </div>

      <div class="controls-overlay">
        <span style="font-size: 12px; font-weight: 600;">Simulate Events:</span>
        <button class="btn btn-secondary" onclick="simulateSafeCommand()">Test Safe Cmd (Score 0.05)</button>
        <button class="btn btn-danger" onclick="simulateDangerousCommand()">Test High Risk (Score 0.95)</button>
        <button class="btn" onclick="simulateEditDiff()">Test AEP Diff & Jev Judge</button>
      </div>
    </div>

    <div class="sidebar">
      <div class="tabs">
        <div class="tab active" onclick="selectTab('events')">Live AG-UI Stream (<span id="event-count">0</span>)</div>
        <div class="tab" onclick="selectTab('decisions')">Jev Inspector</div>
        <div class="tab" onclick="selectTab('diff')">AEP Patch Viewer</div>
      </div>

      <div id="tab-events" class="tab-content active">
        <div id="events-log"></div>
      </div>

      <div id="tab-decisions" class="tab-content">
        <h4 style="margin-bottom: 10px; color: var(--yellow);">Jev Typed Decision State</h4>
        <div style="background: #1e1e1e; padding: 12px; border-radius: 6px; border: 1px solid #333;">
          <div style="display: flex; justify-content: space-between; margin-bottom: 8px;">
            <span>Active Backend:</span>
            <strong id="jev-backend" style="color: var(--cyan);">LocalHeuristic (Free/Offline)</strong>
          </div>
          <div style="display: flex; justify-content: space-between; margin-bottom: 8px;">
            <span>Auto-Allow Threshold:</span>
            <span style="color: var(--green);">&lt; 0.20</span>
          </div>
          <div style="display: flex; justify-content: space-between; margin-bottom: 8px;">
            <span>Interactive Prompt:</span>
            <span style="color: var(--yellow);">0.20 – 0.75</span>
          </div>
          <div style="display: flex; justify-content: space-between; margin-bottom: 8px;">
            <span>Hard Block Threshold:</span>
            <span style="color: var(--red);">&ge; 0.75</span>
          </div>
        </div>
        <div id="decision-history" style="margin-top: 14px;"></div>
      </div>

      <div id="tab-diff" class="tab-content">
        <h4 style="margin-bottom: 10px; color: var(--green);">Agent Editing Protocol (AEP) Diff</h4>
        <div id="aep-diff-display">
          <p style="color: var(--text-muted); font-style: italic;">No patch applied yet. Trigger a simulated edit or run termed agent.</p>
        </div>
      </div>
    </div>
  </div>

  <script>
    let events = [];
    const svg = document.getElementById('graph-canvas');

    function drawEdges() {
      svg.innerHTML = `
        <defs>
          <marker id="arrow" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse">
            <path d="M 0 0 L 10 5 L 0 10 z" fill="#777777"/>
          </marker>
        </defs>
      `;
      connect('node_orch', 'node_coder', 'edge-orch-coder', 'A2A');
      connect('node_coder', 'node_mcp_fs', 'edge-coder-fs', 'MCP');
      connect('node_coder', 'node_gate', 'edge-coder-gate', 'Jev Decision');
      connect('node_gate', 'node_mcp_sh', 'edge-gate-sh', 'MCP (Gated)');
      connect('node_coder', 'node_judge', 'edge-coder-judge', 'Jev Judge');
    }

    function connect(fromId, toId, lineId, label) {
      const elFrom = document.getElementById(fromId);
      const elTo = document.getElementById(toId);
      if (!elFrom || !elTo) return;
      const r1 = elFrom.getBoundingClientRect();
      const r2 = elTo.getBoundingClientRect();
      const p = svg.getBoundingClientRect();

      const x1 = r1.left + r1.width/2 - p.left;
      const y1 = r1.top + r1.height/2 - p.top;
      const x2 = r2.left + r2.width/2 - p.left;
      const y2 = r2.top + r2.height/2 - p.top;

      const path = document.createElementNS('http://www.w3.org/2000/svg', 'line');
      path.setAttribute('id', lineId);
      path.setAttribute('class', 'edge-line');
      path.setAttribute('x1', x1);
      path.setAttribute('y1', y1);
      path.setAttribute('x2', x2);
      path.setAttribute('y2', y2);
      path.setAttribute('marker-end', 'url(#arrow)');
      svg.appendChild(path);
    }

    window.addEventListener('resize', drawEdges);
    setTimeout(drawEdges, 100);

    function selectTab(name) {
      document.querySelectorAll('.tab').forEach(t => t.classList.remove('active'));
      document.querySelectorAll('.tab-content').forEach(c => c.classList.remove('active'));
      event.target.classList.add('active');
      document.getElementById('tab-' + name).classList.add('active');
    }

    function appendEvent(frame) {
      events.unshift(frame);
      document.getElementById('event-count').textContent = events.length;
      const container = document.getElementById('events-log');
      const div = document.createElement('div');
      div.className = 'event-entry';
      let cls = 'event-type';
      if (frame.event_type.includes('decision')) cls += ' decision';
      else if (frame.event_type.includes('patch')) cls += ' patch';
      else if (frame.event_type.includes('tool')) cls += ' tool';

      div.innerHTML = `
        <div class="${cls}">[${frame.event_type}] ${frame.session_id}</div>
        <div style="color: #aaa; margin-bottom: 2px;">${new Date(frame.timestamp_ms).toLocaleTimeString()}</div>
        <pre style="white-space: pre-wrap; word-break: break-all; color: #ddd;">${JSON.stringify(frame.payload, null, 2)}</pre>
      `;
      container.prepend(div);

      // Pulse nodes
      if (frame.event_type.includes('decision')) {
        highlightNode('node_gate');
        addDecisionRecord(frame.payload);
      } else if (frame.event_type.includes('patch')) {
        highlightNode('node_mcp_fs');
        renderPatch(frame.payload);
      } else if (frame.event_type.includes('tool')) {
        if (frame.payload.tool_name && frame.payload.tool_name.includes('command')) {
          highlightNode('node_mcp_sh');
        } else {
          highlightNode('node_mcp_fs');
        }
      }
    }

    function highlightNode(id) {
      const el = document.getElementById(id);
      if (!el) return;
      el.classList.add('active-pulse');
      setTimeout(() => el.classList.remove('active-pulse'), 1500);
    }

    function addDecisionRecord(payload) {
      const hist = document.getElementById('decision-history');
      const div = document.createElement('div');
      div.style.cssText = 'background: #1e1e1e; border: 1px solid #333; border-radius: 6px; padding: 10px; margin-bottom: 8px;';
      div.innerHTML = `
        <div style="font-weight: 600; color: var(--yellow);">${payload.query || payload.metric || 'Decision Evaluation'}</div>
        <div style="font-size: 11px; color: #888; margin: 4px 0;">Target: ${payload.target || payload.command || 'N/A'}</div>
        <div style="font-size: 12px; margin-top: 4px;">
          <span>Verdict: </span>
          <strong style="color: ${payload.score < 0.2 ? 'var(--green)' : payload.score >= 0.75 ? 'var(--red)' : 'var(--yellow)'}">
            ${payload.verdict || (payload.score < 0.2 ? 'Auto-Allow' : payload.score >= 0.75 ? 'Hard Block' : 'Prompt User')}
          </strong>
          ${payload.score !== undefined ? `(Score: ${payload.score.toFixed(2)})` : ''}
        </div>
      `;
      hist.prepend(div);
    }

    function renderPatch(payload) {
      const box = document.getElementById('aep-diff-display');
      box.innerHTML = `
        <div style="background: #1e1e1e; border: 1px solid #333; border-radius: 6px; padding: 10px;">
          <div style="font-weight: 600; color: var(--cyan);">${payload.path || 'src/calc.py'}</div>
          <div class="diff-box">
            <span class="diff-del">- ${payload.old_text || 'return a - b'}</span>
            <span class="diff-add">+ ${payload.new_text || 'return a + b'}</span>
          </div>
          <div style="font-size: 11px; color: #888; margin-top: 6px;">
            Applied via AEP (Agent Editing Protocol) • Jev-Judge verified
          </div>
        </div>
      `;
    }

    // Connect to live SSE stream
    try {
      const evtSource = new EventSource('/events');
      evtSource.onmessage = function(e) {
        try {
          const frame = JSON.parse(e.data);
          appendEvent(frame);
        } catch(err) {}
      };
      evtSource.onerror = function() {
        document.getElementById('conn-dot').classList.add('disconnected');
        document.getElementById('conn-status').textContent = 'AG-UI Stream Offline (Simulation Active)';
      };
      evtSource.onopen = function() {
        document.getElementById('conn-dot').classList.remove('disconnected');
        document.getElementById('conn-status').textContent = 'Connected to AG-UI Stream';
      };
    } catch(e) {
      document.getElementById('conn-dot').classList.add('disconnected');
    }

    // Interactive Simulators for manual test & demonstration
    function simulateSafeCommand() {
      appendEvent({
        event_id: 'evt_' + Math.random().toString(36).substr(2, 9),
        timestamp_ms: Date.now(),
        session_id: 'sess_live',
        event_type: 'decision:evaluated',
        payload: {
          metric: 'shell_execution_risk',
          command: 'cargo check',
          score: 0.05,
          uncertainty: 0.05,
          verdict: 'AutoAllow',
          backend: 'LocalHeuristicBackend'
        }
      });
      setTimeout(() => {
        appendEvent({
          event_id: 'evt_' + Math.random().toString(36).substr(2, 9),
          timestamp_ms: Date.now(),
          session_id: 'sess_live',
          event_type: 'tool:completed',
          payload: {
            tool_name: 'run_command',
            command: 'cargo check',
            exit_code: 0,
            stdout: 'Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s'
          }
        });
      }, 300);
    }

    function simulateDangerousCommand() {
      appendEvent({
        event_id: 'evt_' + Math.random().toString(36).substr(2, 9),
        timestamp_ms: Date.now(),
        session_id: 'sess_live',
        event_type: 'decision:evaluated',
        payload: {
          metric: 'shell_execution_risk',
          command: 'rm -rf src/',
          score: 0.95,
          uncertainty: 0.05,
          verdict: 'Hard Block',
          rationale: 'Critical signature matched: recursive deletion of workspace source code',
          backend: 'LocalHeuristicBackend'
        }
      });
    }

    function simulateEditDiff() {
      appendEvent({
        event_id: 'evt_' + Math.random().toString(36).substr(2, 9),
        timestamp_ms: Date.now(),
        session_id: 'sess_live',
        event_type: 'patch:applied',
        payload: {
          path: 'src/calc.py',
          old_text: 'return a - b',
          new_text: 'return a + b',
          hunks: 1,
          protocol: 'AEP'
        }
      });
      setTimeout(() => {
        appendEvent({
          event_id: 'evt_' + Math.random().toString(36).substr(2, 9),
          timestamp_ms: Date.now(),
          session_id: 'sess_live',
          event_type: 'decision:evaluated',
          payload: {
            query: 'jev-judge: Did patch resolve bug without syntax regressions?',
            judgment: true,
            confidence: 0.94,
            backend: 'LocalHeuristicBackend'
          }
        });
        highlightNode('node_judge');
      }, 400);
    }
  </script>
</body>
</html>
"##;
