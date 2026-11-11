"use strict";

const byId = (id) => document.getElementById(id);
const catalog = byId("catalog");
const access = byId("access");
const token = byId("token");
const details = byId("details");
const quote = byId("quote");
const status = byId("status");
const total = byId("total");
const currency = new Intl.NumberFormat("ja-JP", {
  style: "currency",
  currency: "USD",
});

let selection = null;
let revision = 0;

async function jsonRequest(path, options = {}) {
  const response = await fetch(path, options);
  const result = await response.json();
  if (!response.ok) {
    throw new Error(result.error ?? `HTTP ${response.status}`);
  }
  return result;
}

function authorization() {
  return { Authorization: `Bearer ${token.value}` };
}

function itemPath(sku) {
  return `/api/items/${encodeURIComponent(sku)}`;
}

async function loadItem() {
  if (selection === null || !token.checkValidity()) {
    return;
  }
  const current = ++revision;
  const path = itemPath(selection);
  const headers = authorization();
  details.hidden = true;
  quote.hidden = true;
  total.value = "";
  status.textContent = "読み込み中";
  try {
    const [item, stock] = await Promise.all([
      jsonRequest(path, { headers }),
      jsonRequest(`${path}/stock`, { headers }),
    ]);
    if (current !== revision) {
      return;
    }
    byId("name").textContent = item.name;
    byId("description").textContent = item.description;
    byId("price").textContent = currency.format(item.priceCents / 100);
    byId("stock").textContent = `${stock.available} 点（${stock.warehouse}）`;
    details.hidden = false;
    quote.hidden = false;
    status.textContent = "";
  } catch (error) {
    if (current === revision) {
      status.textContent = error.message;
    }
  }
}

function select(button, sku) {
  selection = sku;
  revision += 1;
  details.hidden = true;
  quote.hidden = true;
  total.value = "";
  for (const item of catalog.querySelectorAll("button")) {
    item.setAttribute("aria-pressed", String(item === button));
  }
  if (token.checkValidity()) {
    void loadItem();
  } else {
    status.textContent = "トークンを入力して詳細を取得してください。";
  }
}

async function loadCatalog() {
  try {
    const { items } = await jsonRequest("/api/catalog?limit=8");
    for (const item of items) {
      const row = document.createElement("li");
      const button = document.createElement("button");
      button.type = "button";
      button.textContent = item.name;
      button.setAttribute("aria-pressed", "false");
      button.addEventListener("click", () => select(button, item.sku));
      row.append(button);
      catalog.append(row);
    }
    if (items.length > 0) {
      select(catalog.querySelector("button"), items[0].sku);
    }
  } catch (error) {
    status.textContent = error.message;
  }
}

access.addEventListener("submit", (event) => {
  event.preventDefault();
  void loadItem();
});

token.addEventListener("input", () => {
  revision += 1;
  details.hidden = true;
  quote.hidden = true;
  total.value = "";
  status.textContent = "";
});

quote.addEventListener("submit", async (event) => {
  event.preventDefault();
  if (selection === null || !token.checkValidity()) {
    return;
  }
  const current = revision;
  try {
    const result = await jsonRequest(`${itemPath(selection)}/quote`, {
      method: "POST",
      headers: { ...authorization(), "Content-Type": "application/json" },
      body: JSON.stringify({ quantity: byId("quantity").valueAsNumber }),
    });
    if (current === revision) {
      total.value = currency.format(result.totalCents / 100);
    }
  } catch (error) {
    if (current === revision) {
      total.value = error.message;
    }
  }
});

void loadCatalog();
