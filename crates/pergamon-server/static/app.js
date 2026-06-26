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
})();
