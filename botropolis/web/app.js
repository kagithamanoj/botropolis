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
  var tabRoster = document.getElementById("tab-roster");
  var viewChat = document.getElementById("view-chat");
  var viewRoster = document.getElementById("view-roster");
  var rosterList = document.getElementById("roster-list");
  var deptDetail = document.getElementById("dept-detail");
  var agentDetail = document.getElementById("agent-detail");

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

  function showTab(which) {
    var chat = which === "chat";
    tabChat.classList.toggle("active", chat);
    tabRoster.classList.toggle("active", !chat);
    viewChat.classList.toggle("hidden", !chat);
    viewRoster.classList.toggle("hidden", chat);
    if (!chat && !rosterList.dataset.loaded) {
      loadDepartments();
    }
  }

  tabChat.addEventListener("click", function () { showTab("chat"); });
  tabRoster.addEventListener("click", function () { showTab("roster"); });

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

  function renderReport(report) {
    var wrap = document.createElement("div");
    wrap.className = "msg company";

    var html = '<div class="report">';
    html += '<div class="ceo-label">CEO summary</div>';
    html += '<p class="summary">' + esc(report.summary || "Done.") + "</p>";

    if (report.departments_involved && report.departments_involved.length) {
      html += '<div class="dept-chips">';
      report.departments_involved.forEach(function (d) {
        html += '<span class="chip">' + esc(d) + "</span>";
      });
      html += "</div>";
    }

    (report.results || []).forEach(function (r) {
      html += '<div class="agent-card">';
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
      html += "</div>";
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
      wrap.innerHTML = '<div class="report"><div class="ceo-label">CEO</div>' +
        '<p class="summary">Welcome to Botropolis. Ask me anything and I will route it ' +
        "to the right departments.</p></div>";
      messagesEl.appendChild(wrap);
    })
    .catch(function () {
      setStatus("Offline - could not reach the server", "bad");
    });
})();
