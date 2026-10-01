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
  var tabWarroom = document.getElementById("tab-warroom");
  var tabAnalytics = document.getElementById("tab-analytics");
  var tabRoster = document.getElementById("tab-roster");
  var viewChat = document.getElementById("view-chat");
  var viewWarroom = document.getElementById("view-warroom");
  var viewAnalytics = document.getElementById("view-analytics");
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
    warroom: { tab: tabWarroom, view: viewWarroom },
    analytics: { tab: tabAnalytics, view: viewAnalytics },
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
    if (which === "warroom" && !warroomAgent.dataset.loaded) {
      loadWarroomAgents();
    }
    if (which === "analytics") {
      loadAnalytics();
    }
  }

  Object.keys(TABS).forEach(function (key) {
    TABS[key].tab.addEventListener("click", function () { showTab(key); });
  });

  /* ---- Chat ---- */

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

    fetch("/ask", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ request: text })
    })
      .then(function (resp) {
        if (!resp.ok) throw new Error("server returned " + resp.status);
        return resp.json();
      })
      .then(function (report) {
        removeTyping();
        renderReport(report);
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

  /* ---- War room: direct chat with one agent ---- */

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

  function addWarroomTyping() {
    var wrap = document.createElement("div");
    wrap.className = "msg company";
    wrap.id = "warroom-typing";
    wrap.innerHTML = '<div class="typing"><span></span><span></span><span></span></div>';
    warroomMessages.appendChild(wrap);
  }

  function removeWarroomTyping() {
    var row = document.getElementById("warroom-typing");
    if (row) row.remove();
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
    addWarroomTyping();

    fetch("/agents/" + encodeURIComponent(name) + "/ask", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ request: text })
    })
      .then(function (resp) {
        if (!resp.ok) throw new Error("server returned " + resp.status);
        return resp.json();
      })
      .then(function (result) {
        removeWarroomTyping();
        addWarroomResult(name, result);
      })
      .catch(function (err) {
        removeWarroomTyping();
        addWarroomNote("Something went wrong: " + err.message);
      })
      .finally(function () {
        warroomSend.disabled = false;
        warroomInput.focus();
      });
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
        html += "<tr><th>Agent</th><th>Calls</th><th>Avg ms</th><th>Tokens</th><th>Stub runs</th></tr>";
        names.forEach(function (n) {
          var a = agents[n];
          html += "<tr><td>" + esc(n) + "</td><td>" + a.calls + "</td><td>" +
            a.avg_latency_ms + "</td><td>" + a.tokens_total + "</td><td>" +
            a.stub_calls + "</td></tr>";
        });
        html += "</table>";
        html += '<p class="totals-line">Total calls: ' + totals.calls +
          " - stub runs: " + totals.stub_calls + "</p>";
        analyticsTable.innerHTML = html;
      })
      .catch(function () {
        analyticsTable.innerHTML = "<p>Could not load analytics.</p>";
      });
  }

  analyticsRefresh.addEventListener("click", loadAnalytics);

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
