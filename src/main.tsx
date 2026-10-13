import { render } from "solid-js/web";
import "./styles/app.css";
import App from "./ui/App";
import { actions } from "./state/store";

render(() => <App />, document.getElementById("root")!);
void actions.boot();
