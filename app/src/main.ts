import { mount } from "svelte";
import App from "./App.svelte";
import { createApi } from "./lib/api";
import { AppState } from "./lib/app.svelte";
import "./styles/tokens.css";
import "./styles/app.css";

const app = new AppState(await createApi());
mount(App, { target: document.getElementById("app")!, props: { app } });
