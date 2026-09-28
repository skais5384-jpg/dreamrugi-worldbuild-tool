import React from "react";
import ReactDOM from "react-dom/client";
import App from "./app/WorkspaceApp";
import ErrorBoundary from "./app/ErrorBoundary";
import "./styles/global.css";
import { text } from "./strings";
import { AppProvider } from "./ui/AppProvider";

document.title = text("app.message02");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <AppProvider>
        <ErrorBoundary fluent>
          <App />
        </ErrorBoundary>
      </AppProvider>
    </ErrorBoundary>
  </React.StrictMode>,
);
