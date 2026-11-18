import { mount } from "svelte";
import "./app.css";
import App from "./App.svelte";
import { theme } from "./lib/theme.svelte";

const target = document.getElementById("app");
if (!target) throw new Error("#app not found");

// The theme is on <html> before the first paint.
theme.start();

export default mount(App, { target });
