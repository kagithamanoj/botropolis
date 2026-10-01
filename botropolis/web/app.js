/* Botropolis web UI. Vanilla JS, no build step. */

(function () {
  "use strict";

  var STUB_MARKER = "offline stub";

  var messagesEl = document.getElementById("messages");
  var formEl = document.getElementById("ask-form");
  var inputEl = document.getElementById("ask-input");
  var sendBtn = document.getElementById("ask-send");
  var statusEl = document.getElementById("status");
  var tabChat = document.getElementById("tab-chat");
  var tabTeam = document.getElementById("tab-team");
  var tabWarroom = document.getElementById("tab-warroom");
  var tabAnalytics = document.getElementById("tab-analytics");
  var tabNotebook = document.getElementById("tab-notebook");
  var tabRoster = document.getElementById("tab-roster");
  var viewChat = document.getElementById("view-chat");
  var viewTeam = document.getElementById("view-team");
  var viewWarroom = document.getElementById("view-warroom");
  var viewAnalytics = document.getElementById("view-analytics");
  var viewNotebook = document.getElementById("view-notebook");
  var viewRoster = document.getElementById("view-roster");
  var rosterList = document.getElementById("roster-list");
  var deptDetail = document.getElementById("dept-detail");
  var agentDetail = document.getElementById("agent-detail");
  var warroomAgent = document.getElementById("warroom-agent");
  var warroomMessages = document.getElementById("warroom-messages");
  var warroomForm = document.getElementById("warroom-form");
  var warroomInput = document.getElementById("warroom-input");
  var warroomSend = document.getElementById("warroom-send");
  var analyticsTable = document.getElementById("analytics-table");
  var analyticsRefresh = document.getElementById("analytics-refresh");
  var teamAgents = document.getElementById("team-agents");
  var teamRounds = document.getElementById("team-rounds");
  var teamResults = document.getElementById("team-results");
  var teamForm = document.getElementById("team-form");
  var teamInput = document.getElementById("team-input");
  var teamSend = document.getElementById("team-send");

  /* Escape user and model text before injecting into the DOM. */
  function esc(text) {
    var div = document.createElement("div");
    div.textContent = text == null ? "" : String(text);
    return div.innerHTML;
  }

  function setStatus(text, cls) {
    statusEl.textContent = text;
    statusEl.className = cls || "";
  }

  /* ---- Tabs ---- */

  var TABS = {
    chat: { tab: tabChat, view: viewChat },
    team: { tab: tabTeam, view: viewTeam },
    warroom: { tab: tabWarroom, view: viewWarroom },
    analytics: { tab: tabAnalytics, view: viewAnalytics },
    notebook: { tab: tabNotebook, view: viewNotebook },
    roster: { tab: tabRoster, view: viewRoster }
  };

  function showTab(which) {
    Object.keys(TABS).forEach(function (key) {
      var on = key === which;
      TABS[key].tab.classList.toggle("active", on);
      TABS[key].view.classList.toggle("hidden", !on);
    });
    if (which === "roster" && !rosterList.dataset.loaded) {
      loadDepartments();
    }
    if (which === "team" && !teamAgents.dataset.loaded) {
      loadTeamAgents();
    }
    if (which === "warroom" && !warroomAgent.dataset.loaded) {
      loadWarroomAgents();
    }
    if (which === "analytics") {
      loadAnalytics();
    }
    if (which === "notebook") {
      loadNotebook();
    }
  }

  Object.keys(TABS).forEach(function (key) {
    TABS[key].tab.addEventListener("click", function () { showTab(key); });
  });

  /* ---- Chat ---- */

  /* Recent turns sent with each request so follow-up questions have
     context. Capped; Clear wipes it. */
  var chatHistory = [];
  var CHAT_HISTORY_TURNS = 10;

  function pushChatTurn(role, content) {
    chatHistory.push({ role: role, content: content });
    while (chatHistory.length > CHAT_HISTORY_TURNS) chatHistory.shift();
  }

  var askClear = document.getElementById("ask-clear");
  askClear.addEventListener("click", function () {
    chatHistory = [];
    messagesEl.innerHTML = "";
    inputEl.focus();
  });

  function addUserMessage(text) {
    var wrap = document.createElement("div");
    wrap.className = "msg user";
    wrap.innerHTML = '<div class="bubble">' + esc(text) + "</div>";
    messagesEl.appendChild(wrap);
    scrollDown();
  }

  function addTyping() {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.id = "typing-row";
    wrap.innerHTML = '<div class="typing"><span></span><span></span><span></span></div>';
    messagesEl.appendChild(wrap);
    scrollDown();
  }

  function removeTyping() {
    var row = document.getElementById("typing-row");
    if (row) row.remove();
  }

  function isStub(output) {
    return output.toLowerCase().indexOf(STUB_MARKER) !== -1;
  }

  function agentCardHtml(r) {
    var html = '<div class="agent-card">';
    html += '<div class="agent-head"><span class="agent-name">' + esc(r.agent_name) +
            '</span><span class="agent-dept">' + esc(r.department) + "</span></div>";
    if (!r.success) {
      html += '<p class="output">Failed: ' + esc(r.error || "unknown error") + "</p>";
    } else {
      if (isStub(r.output || "")) {
        html += '<span class="stub-badge">offline stub</span>';
      }
      html += '<p class="output">' + esc(r.output) + "</p>";
    }
    if (r.tool_calls && r.tool_calls.length) {
      html += '<div class="tool-calls">';
      r.tool_calls.forEach(function (tc) {
        var status = tc.success ? "ok" : "failed";
        html += '<div class="tool-call"><span class="tool-name">' + esc(tc.tool) + "</span> " +
          '<span class="tool-args">' + esc(JSON.stringify(tc.args || {})) + "</span> " +
          '<span class="tool-status ' + status + '">' + status + "</span></div>";
      });
      html += "</div>";
    }
    html += "</div>";
    return html;
  }

  function renderReport(report) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";

    var html = '<div class="report">';
    html += '<div class="ceo-label">Manoj\'s summary</div>';
    html += '<p class="summary">' + esc(report.summary || "Done.") + "</p>";

    if (report.departments_involved && report.departments_involved.length) {
      html += '<div class="dept-chips">';
      report.departments_involved.forEach(function (d) {
        html += '<span class="chip">' + esc(d) + "</span>";
      });
      html += "</div>";
    }

    (report.results || []).forEach(function (r) {
      html += agentCardHtml(r);
    });

    html += "</div>";
    wrap.innerHTML = html;
    messagesEl.appendChild(wrap);
    scrollDown();
  }

  function addError(text) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.innerHTML = '<div class="report"><p class="summary">Something went wrong: ' +
      esc(text) + "</p></div>";
    messagesEl.appendChild(wrap);
    scrollDown();
  }

  function scrollDown() {
    window.scrollTo({ top: document.body.scrollHeight, behavior: "smooth" });
  }

  formEl.addEventListener("submit", function (ev) {
    ev.preventDefault();
    var text = inputEl.value.trim();
    if (!text) return;
    inputEl.value = "";
    sendBtn.disabled = true;
    addUserMessage(text);
    addTyping();
    pushChatTurn("user", text);

    fetch("/ask", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ request: text, history: chatHistory.slice(0, -1) })
    })
      .then(function (resp) {
        if (!resp.ok) throw new Error("server returned " + resp.status);
        return resp.json();
      })
      .then(function (report) {
        removeTyping();
        renderReport(report);
        pushChatTurn("assistant", report.summary || "Done.");
      })
      .catch(function (err) {
        removeTyping();
        addError(err.message);
      })
      .finally(function () {
        sendBtn.disabled = false;
        inputEl.focus();
      });
  });

  /* ---- Teaming: run a team of agents in collaboration rounds ---- */

  function loadTeamAgents() {
    fetch("/agents")
      .then(function (resp) { return resp.json(); })
      .then(function (data) {
        teamAgents.dataset.loaded = "1";
        teamAgents.innerHTML = "";
        data.agents.forEach(function (a) {
          var label = document.createElement("label");
          label.className = "team-agent";
          var cb = document.createElement("input");
          cb.type = "checkbox";
          cb.value = a.name;
          label.appendChild(cb);
          var span = document.createElement("span");
          span.textContent = a.name + " - " + a.title;
          label.appendChild(span);
          teamAgents.appendChild(label);
        });
      })
      .catch(function () {
        teamAgents.innerHTML = "<p>Could not load agents.</p>";
      });
  }

  function addTeamNote(text) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.innerHTML = '<div class="report"><div class="ceo-label">Teaming</div>' +
      '<p class="summary">' + esc(text) + "</p></div>";
    teamResults.appendChild(wrap);
    scrollDown();
  }

  function addTeamTyping() {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.id = "team-typing";
    wrap.innerHTML = '<div class="typing"><span></span><span></span><span></span></div>';
    teamResults.appendChild(wrap);
    scrollDown();
  }

  function removeTeamTyping() {
    var row = document.getElementById("team-typing");
    if (row) row.remove();
  }

  function renderTeamReport(report) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";

    var html = '<div class="report">';
    html += '<div class="ceo-label">Team session: ' + esc((report.agents || []).join(", ")) + "</div>";

    var byRound = {};
    (report.rounds || []).forEach(function (rd) {
      var key = rd.round_number;
      if (!byRound[key]) byRound[key] = [];
      byRound[key].push(rd);
    });
    Object.keys(byRound).sort(function (a, b) { return a - b; }).forEach(function (n) {
      html += '<div class="team-round"><div class="round-label">Round ' + esc(n) + "</div>";
      byRound[n].forEach(function (rd) {
        html += agentCardHtml(rd.result);
      });
      html += "</div>";
    });

    html += '<div class="ceo-label">Manoj\'s synthesis</div>';
    html += '<p class="summary">' + esc(report.synthesis || "Done.") + "</p>";
    html += "</div>";
    wrap.innerHTML = html;
    teamResults.appendChild(wrap);
    scrollDown();
  }

  teamForm.addEventListener("submit", function (ev) {
    ev.preventDefault();
    var text = teamInput.value.trim();
    var agents = [];
    teamAgents.querySelectorAll("input[type=checkbox]").forEach(function (cb) {
      if (cb.checked) agents.push(cb.value);
    });
    var rounds = parseInt(teamRounds.value, 10) || 2;
    if (!text || !agents.length) {
      addTeamNote("Pick at least one agent and describe the task first.");
      return;
    }
    teamInput.value = "";
    teamSend.disabled = true;
    addTeamNote("Running " + agents.join(", ") + " for " + rounds +
      (rounds === 1 ? " round." : " rounds."));
    addTeamTyping();

    fetch("/team", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ request: text, agents: agents, rounds: rounds })
    })
      .then(function (resp) {
        if (!resp.ok) {
          return resp.json().then(function (body) {
            throw new Error(body.detail || ("server returned " + resp.status));
          });
        }
        return resp.json();
      })
      .then(function (report) {
        removeTeamTyping();
        renderTeamReport(report);
      })
      .catch(function (err) {
        removeTeamTyping();
        addTeamNote("Something went wrong: " + err.message);
      })
      .finally(function () {
        teamSend.disabled = false;
        teamInput.focus();
      });
  });

  /* ---- War room: direct chat with one agent ---- */

  /* Per-agent conversation memory, mirroring the chat tab. Reset when
     the picked agent changes. */
  var warroomHistory = [];
  var WARROOM_HISTORY_TURNS = 10;

  function pushWarroomTurn(role, content) {
    warroomHistory.push({ role: role, content: content });
    while (warroomHistory.length > WARROOM_HISTORY_TURNS) warroomHistory.shift();
  }

  warroomAgent.addEventListener("change", function () {
    warroomHistory = [];
  });

  function loadWarroomAgents() {
    fetch("/agents")
      .then(function (resp) { return resp.json(); })
      .then(function (data) {
        warroomAgent.dataset.loaded = "1";
        warroomAgent.innerHTML = "";
        data.agents.forEach(function (a) {
          var opt = document.createElement("option");
          opt.value = a.name;
          opt.textContent = a.name + " - " + a.title;
          warroomAgent.appendChild(opt);
        });
        addWarroomNote("Pick an agent and ask. This goes straight to them, no routing through Manoj.");
      })
      .catch(function () {
        warroomAgent.innerHTML = "";
        addWarroomNote("Could not load agents.");
      });
  }

  function addWarroomNote(text) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.innerHTML = '<div class="report"><div class="ceo-label">War room</div>' +
      '<p class="summary">' + esc(text) + "</p></div>";
    warroomMessages.appendChild(wrap);
  }

  function addWarroomUser(text) {
    var wrap = document.createElement("div");
    wrap.className = "msg user";
    wrap.innerHTML = '<div class="bubble">' + esc(text) + "</div>";
    warroomMessages.appendChild(wrap);
  }

  function addWarroomResult(name, r) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.innerHTML = '<div class="report"><div class="ceo-label">' + esc(name) +
      "</div>" + agentCardHtml(r) + "</div>";
    warroomMessages.appendChild(wrap);
  }

  warroomForm.addEventListener("submit", function (ev) {
    ev.preventDefault();
    var text = warroomInput.value.trim();
    var name = warroomAgent.value;
    if (!text || !name) return;
    warroomInput.value = "";
    warroomSend.disabled = true;
    addWarroomUser(text);

    /* Live card: tool calls appear here as the agent works, then the
       full result card replaces it. */
    var liveWrap = document.createElement("div");
    liveWrap.className = "msg company";
    liveWrap.innerHTML = '<div class="report"><div class="ceo-label">' + esc(name) +
      ' <span class="live-dot">live</span></div><div class="tool-calls"></div></div>';
    warroomMessages.appendChild(liveWrap);
    scrollDown();
    var liveList = liveWrap.querySelector(".tool-calls");
    var callCount = 0;
    var finished = false;

    var src = new EventSource("/agents/" + encodeURIComponent(name) +
      "/ask/stream?request=" + encodeURIComponent(text) +
      "&history=" + encodeURIComponent(JSON.stringify(warroomHistory)));
    pushWarroomTurn("user", text);

    function close() {
      if (!finished) {
        finished = true;
        src.close();
        warroomSend.disabled = false;
        warroomInput.focus();
      }
    }

    function fail(msg) {
      liveWrap.remove();
      addWarroomNote("Something went wrong: " + msg);
      close();
    }

    src.addEventListener("tool_started", function (e) {
      var data = JSON.parse(e.data);
      callCount += 1;
      var div = document.createElement("div");
      div.className = "tool-call";
      div.id = "live-call-" + callCount;
      div.innerHTML = '<span class="tool-name">' + esc(data.tool) + "</span> " +
        '<span class="tool-args">' + esc(JSON.stringify(data.args || {})) + "</span> " +
        '<span class="tool-status running">running</span>';
      liveList.appendChild(div);
      scrollDown();
    });

    src.addEventListener("tool_finished", function () {
      var div = document.getElementById("live-call-" + callCount);
      if (div) {
        var badge = div.querySelector(".tool-status");
        /* The full result card carries the final status; here we just
           mark the call as no longer running. */
        badge.className = "tool-status ok";
        badge.textContent = "done";
      }
      scrollDown();
    });

    src.addEventListener("result", function (e) {
      liveWrap.remove();
      var r = JSON.parse(e.data);
      addWarroomResult(name, r);
      pushWarroomTurn("assistant", r.output || "Done.");
      close();
    });

    src.addEventListener("failed", function (e) {
      var msg = "stream failed";
      try { msg = JSON.parse(e.data).message || msg; } catch (err) { /* keep default */ }
      fail(msg);
    });

    src.onerror = function () {
      if (!finished) fail("Lost the stream. The server may be busy; try again.");
    };

    /* If the agent has no toolkit there are no tool events; the stream
       just yields the result, so the live card flashes by quickly. */
  });

  /* ---- Analytics ---- */

  function loadAnalytics() {
    fetch("/analytics")
      .then(function (resp) { return resp.json(); })
      .then(function (data) {
        var agents = data.agents || {};
        var names = Object.keys(agents);
        var totals = data.totals || {};
        if (!names.length) {
          analyticsTable.innerHTML = "<p>No runs recorded yet. Chat or use the war room first.</p>";
          return;
        }
        var html = '<table class="analytics-table">';
        html += "<tr><th>Agent</th><th>Calls</th><th>Avg ms</th><th>Tokens</th><th>Tool calls</th><th>Stub runs</th></tr>";
        names.forEach(function (n) {
          var a = agents[n];
          html += "<tr><td>" + esc(n) + "</td><td>" + a.calls + "</td><td>" +
            a.avg_latency_ms + "</td><td>" + a.tokens_total + "</td><td>" +
            (a.tool_calls || 0) + "</td><td>" + a.stub_calls + "</td></tr>";
        });
        html += "</table>";
        html += '<p class="totals-line">Total calls: ' + totals.calls +
          " - tool calls: " + (totals.tool_calls || 0) +
          " - stub runs: " + totals.stub_calls + "</p>";
        analyticsTable.innerHTML = html;
      })
      .catch(function () {
        analyticsTable.innerHTML = "<p>Could not load analytics.</p>";
      });
  }

  analyticsRefresh.addEventListener("click", loadAnalytics);

  /* ---- Notebook: read-only view of the shared company notebook ---- */

  var notebookContent = document.getElementById("notebook-content");
  var notebookRefresh = document.getElementById("notebook-refresh");

  function loadNotebook() {
    notebookContent.textContent = "Loading...";
    fetch("/notebook")
      .then(function (resp) { return resp.json(); })
      .then(function (data) {
        notebookContent.textContent = data.content ||
          "The notebook is empty. Agents with the notes tools write here.";
      })
      .catch(function () {
        notebookContent.textContent = "Could not load the notebook.";
      });
  }

  notebookRefresh.addEventListener("click", loadNotebook);

  /* ---- Roster ---- */

  function loadDepartments() {
    fetch("/departments")
      .then(function (resp) { return resp.json(); })
      .then(function (data) {
        rosterList.dataset.loaded = "1";
        rosterList.innerHTML = "";
        data.departments.forEach(function (dept) {
          var btn = document.createElement("button");
          btn.type = "button";
          btn.className = "dept-row";
          btn.innerHTML = "<span>" + esc(dept.name) + "</span>" +
            '<span class="count">' + dept.agents.length + " agents</span>";
          btn.addEventListener("click", function () { showDepartment(dept); });
          rosterList.appendChild(btn);
        });
      })
      .catch(function () {
        rosterList.innerHTML = "<p>Could not load departments.</p>";
      });
  }

  function showDepartment(dept) {
    rosterList.classList.add("hidden");
    agentDetail.classList.add("hidden");
    deptDetail.classList.remove("hidden");

    var html = '<button type="button" class="back-btn" id="dept-back">&larr; Departments</button>';
    html += '<div class="detail-card"><h2>' + esc(dept.name) + "</h2>";
    html += '<p class="subtitle">' + dept.agents.length + " agents</p>";
    dept.agents.forEach(function (name) {
      html += '<button type="button" class="agent-row" data-agent="' + esc(name) + '">' +
        "<span>" + esc(name) + "</span></button>";
    });
    html += "</div>";
    deptDetail.innerHTML = html;

    document.getElementById("dept-back").addEventListener("click", function () {
      deptDetail.classList.add("hidden");
      rosterList.classList.remove("hidden");
    });

    deptDetail.querySelectorAll(".agent-row").forEach(function (btn) {
      btn.addEventListener("click", function () {
        showAgent(btn.getAttribute("data-agent"));
      });
    });
  }

  function showAgent(name) {
    fetch("/agents/" + encodeURIComponent(name))
      .then(function (resp) {
        if (!resp.ok) throw new Error("not found");
        return resp.json();
      })
      .then(function (a) {
        deptDetail.classList.add("hidden");
        agentDetail.classList.remove("hidden");

        var html = '<button type="button" class="back-btn" id="agent-back">&larr; Back</button>';
        html += '<div class="detail-card">';
        html += "<h2>" + esc(a.name) + "</h2>";
        html += '<p class="subtitle">' + esc(a.title) + " - " + esc(a.department) + "</p>";
        html += '<table class="spec-table">';
        html += "<tr><th>Specialty</th><td>" + esc(a.specialty) + "</td></tr>";
        html += "<tr><th>Model</th><td>" + esc(a.model) + "</td></tr>";
        html += "<tr><th>Tools</th><td>" + (a.tools || []).map(function (t) {
          return '<span class="tool-tag">' + esc(t) + "</span>";
        }).join("") + "</td></tr>";
        html += "</table>";

        if (a.example_tasks && a.example_tasks.length) {
          html += "<h3>Example tasks</h3>";
          html += '<ul class="example-list">';
          a.example_tasks.forEach(function (t) {
            html += "<li>" + esc(t) + "</li>";
          });
          html += "</ul>";
        }
        html += "</div>";
        agentDetail.innerHTML = html;

        document.getElementById("agent-back").addEventListener("click", function () {
          agentDetail.classList.add("hidden");
          deptDetail.classList.remove("hidden");
        });
      })
      .catch(function () {
        agentDetail.classList.remove("hidden");
        agentDetail.innerHTML = "<p>Could not load agent.</p>";
      });
  }

  /* ---- Boot: health check and welcome ---- */

  fetch("/health")
    .then(function (resp) { return resp.json(); })
    .then(function (data) {
      setStatus("Online - " + data.agents + " agents on staff", "ok");
      var wrap = document.createElement("div");
      wrap.className = "msg company";
      wrap.innerHTML = '<div class="report"><div class="ceo-label">Manoj</div>' +
        '<p class="summary">Welcome to Botropolis. Ask me anything and I will route it ' +
        "to the right departments.</p></div>";
      messagesEl.appendChild(wrap);
    })
    .catch(function () {
      setStatus("Offline - could not reach the server", "bad");
    });
})();
