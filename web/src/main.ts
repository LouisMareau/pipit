import { registerSW } from "virtual:pwa-register";
import { loadSettings } from "./platform/storage";
import { App } from "./ui/app";
import "./ui/styles.css";

registerSW({ immediate: true });

const root = document.getElementById("app");
if (!root) throw new Error("missing #app");

loadSettings().then((settings) => new App(root, settings).start());
