/*
 * pergamon web UI — progressive keyboard shortcuts.
 * SPDX-License-Identifier: AGPL-3.0-only
 *
 * This script is an *enhancement*: every action it triggers is also reachable
 * via a visible link, button, or form, so the UI works fully without JS.
 * Keyboard shortcuts simply activate those same controls.
 */
(function () {
  "use strict";

  function rows() {
    return Array.prototype.slice.call(
      document.querySelectorAll("[data-item-row]")
    );
  }

  var selected = -1;

  function clearSelection() {
    rows().forEach(function (r) {
      r.classList.remove("selected");
    });
  }

  function focusRow(index) {
    var list = rows();
    if (list.length === 0) {
      return;
    }
    selected = Math.max(0, Math.min(index, list.length - 1));
    clearSelection();
    var row = list[selected];
    row.classList.add("selected");
    row.scrollIntoView({ block: "nearest" });
  }

  function currentRow() {
    var list = rows();
    if (selected < 0 || selected >= list.length) {
      return null;
    }
    return list[selected];
  }

  /* Click a control inside the active row by its data-action attribute. */
  function triggerAction(row, action) {
    if (!row) {
      return;
    }
    var el = row.querySelector('[data-action="' + action + '"]');
    if (el) {
      el.click();
    }
  }

  function openOriginal(row) {
    if (!row) {
      return;
    }
    var url = row.getAttribute("data-item-url");
    if (url) {
      window.open(url, "_blank", "noopener");
    }
  }

  function openReader(row) {
    if (!row) {
      return;
    }
    var url = row.getAttribute("data-reader-url");
    if (url) {
      window.location.href = url;
    }
  }

  function toggleSelect(row) {
    if (!row) {
      return;
    }
    var box = row.querySelector('input[type="checkbox"][name="ids"]');
    if (box) {
      box.checked = !box.checked;
    }
  }

  function isTyping(e) {
    var t = e.target;
    if (!t) {
      return false;
    }
    var tag = (t.tagName || "").toLowerCase();
    return (
      tag === "input" ||
      tag === "textarea" ||
      tag === "select" ||
      t.isContentEditable
    );
  }

  document.addEventListener("keydown", function (e) {
    if (isTyping(e) || e.metaKey || e.ctrlKey || e.altKey) {
      return;
    }

    var page = document.body.getAttribute("data-page");

    /* Reader-page shortcuts. */
    if (page === "reader") {
      switch (e.key) {
        case "o":
          var ourl = document.body.getAttribute("data-item-url");
          if (ourl) {
            window.open(ourl, "_blank", "noopener");
          }
          break;
        case "a":
          var ab = document.querySelector('[data-action="archive"]');
          if (ab) {
            ab.click();
          }
          break;
        case "u":
        case "Backspace": {
          var back = document.querySelector("[data-back]");
          if (back) {
            e.preventDefault();
            window.location.href = back.getAttribute("href");
          }
          break;
        }
        default:
          return;
      }
      return;
    }

    /* Review-page shortcuts. */
    if (page === "review") {
      if (e.key === " " || e.key === "Spacebar") {
        var details = document.querySelector(".review-answer");
        if (details) {
          e.preventDefault();
          details.open = !details.open;
        }
        return;
      }
      if (e.key === "1" || e.key === "2" || e.key === "3" || e.key === "4") {
        var ratingButton = document.querySelector(
          '[data-rating="' + e.key + '"]'
        );
        if (ratingButton) {
          e.preventDefault();
          ratingButton.click();
        }
        return;
      }
      return;
    }

    if (page !== "inbox") {
      return;
    }

    /* Inbox / list shortcuts. */
    var row = currentRow();
    switch (e.key) {
      case "j":
        focusRow(selected + 1);
        break;
      case "k":
        focusRow(selected <= 0 ? 0 : selected - 1);
        break;
      case "Enter":
        if (row) {
          e.preventDefault();
          openReader(row);
        }
        break;
      case "o":
        openOriginal(row);
        break;
      case "a":
        triggerAction(row, "archive");
        break;
      case "l":
        triggerAction(row, "later");
        break;
      case "r":
        triggerAction(row, "read");
        break;
      case "s":
        triggerAction(row, "bookmark");
        break;
      case "x":
        toggleSelect(row);
        break;
      default:
        return;
    }
  });

  /* After HTMX swaps the list, keep the selection in range. */
  document.body.addEventListener("htmx:afterSwap", function () {
    if (selected >= 0) {
      focusRow(selected);
    }
  });

  /* ------------------------------------------------------------------
   * Recent searches (client-side, localStorage). Enhancement only: the
   * search box works without this; we just remember recent queries.
   * ---------------------------------------------------------------- */
  (function recentSearches() {
    var KEY = "pergamon.recentSearches";
    var MAX = 8;
    var container = document.querySelector("[data-recent-searches]");
    var list = document.querySelector("[data-recent-search-list]");
    var input = document.querySelector("[data-recent-search-input]");
    if (!container || !list) {
      return;
    }

    function load() {
      try {
        var raw = window.localStorage.getItem(KEY);
        var arr = raw ? JSON.parse(raw) : [];
        return Array.isArray(arr) ? arr : [];
      } catch (e) {
        return [];
      }
    }

    function save(arr) {
      try {
        window.localStorage.setItem(KEY, JSON.stringify(arr.slice(0, MAX)));
      } catch (e) {
        /* ignore quota / disabled storage */
      }
    }

    function record(q) {
      q = (q || "").trim();
      if (!q) {
        return;
      }
      var arr = load().filter(function (x) {
        return x !== q;
      });
      arr.unshift(q);
      save(arr);
    }

    function runSearch(q) {
      if (!input) {
        return;
      }
      input.value = q;
      if (window.htmx) {
        window.htmx.trigger(input, "search");
      } else {
        var form = input.closest("form");
        if (form) {
          form.submit();
        }
      }
    }

    function render() {
      var arr = load();
      list.innerHTML = "";
      if (arr.length === 0) {
        container.hidden = true;
        return;
      }
      container.hidden = false;
      arr.forEach(function (q) {
        var li = document.createElement("li");
        var btn = document.createElement("button");
        btn.type = "button";
        btn.className = "secondary outline";
        btn.textContent = q;
        btn.addEventListener("click", function () {
          runSearch(q);
        });
        li.appendChild(btn);
        list.appendChild(li);
      });
    }

    if (input) {
      /* Record the query a moment after the user stops typing. */
      var timer = null;
      input.addEventListener("input", function () {
        window.clearTimeout(timer);
        var q = input.value;
        timer = window.setTimeout(function () {
          record(q);
          render();
        }, 800);
      });
    }

    render();
  })();

  /* ------------------------------------------------------------------
   * Drag-and-drop reordering for manual collections. Enhancement only:
   * "move up / move down" forms provide the same capability without JS.
   * ---------------------------------------------------------------- */
  (function reorder() {
    var list = document.querySelector("[data-reorder-list]");
    if (!list) {
      return;
    }
    var url = list.getAttribute("data-reorder-url");
    var dragging = null;

    function items() {
      return Array.prototype.slice.call(
        list.querySelectorAll("[data-reorder-item]")
      );
    }

    function persist() {
      if (!url) {
        return;
      }
      var ids = items().map(function (el) {
        return el.getAttribute("data-item-id");
      });
      var body = ids
        .map(function (id) {
          return "ids=" + encodeURIComponent(id);
        })
        .join("&");
      var headers = { "Content-Type": "application/x-www-form-urlencoded" };
      if (window.htmx) {
        headers["HX-Request"] = "true";
      }
      fetch(url, { method: "POST", headers: headers, body: body }).catch(
        function () {
          /* On failure, reload to restore the server's order. */
          window.location.reload();
        }
      );
    }

    list.addEventListener("dragstart", function (e) {
      var row = e.target.closest("[data-reorder-item]");
      if (!row) {
        return;
      }
      dragging = row;
      row.classList.add("dragging");
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", row.getAttribute("data-item-id"));
      }
    });

    list.addEventListener("dragend", function () {
      if (dragging) {
        dragging.classList.remove("dragging");
      }
      items().forEach(function (el) {
        el.classList.remove("drag-over");
      });
      dragging = null;
    });

    list.addEventListener("dragover", function (e) {
      e.preventDefault();
      var over = e.target.closest("[data-reorder-item]");
      if (!over || over === dragging || !dragging) {
        return;
      }
      items().forEach(function (el) {
        el.classList.remove("drag-over");
      });
      over.classList.add("drag-over");
      var rect = over.getBoundingClientRect();
      var after = e.clientY > rect.top + rect.height / 2;
      if (after) {
        over.parentNode.insertBefore(dragging, over.nextSibling);
      } else {
        over.parentNode.insertBefore(dragging, over);
      }
    });

    list.addEventListener("drop", function (e) {
      e.preventDefault();
      items().forEach(function (el) {
        el.classList.remove("drag-over");
      });
      persist();
    });
  })();
})();
