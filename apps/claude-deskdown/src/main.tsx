// ClaudeDeskDown entry point (claude-deskdown.html). Separate from Codex++ main.tsx: own UI, own styles, only reuses backend commands.
import "./legacy-webkit";
import { Component, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { ClaudeDeskDownApp } from "./ClaudeDeskDownApp";
import "./claude-deskdown.css";

class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: unknown) {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error("[ClaudeDeskDown] UI render failed", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="cm-crash" role="alert">
        <h1>ClaudeDeskDown UI failed to load</h1>
        <pre>{error.stack || error.message}</pre>
        <button className="cm-btn" onClick={() => window.location.reload()} type="button">Reload</button>
      </div>
    );
  }
}

// macOS hides system titlebar, traffic lights float at sidebar top-left; sidebar top needs spacing.
if (/Mac/i.test(navigator.platform || navigator.userAgent)) document.documentElement.classList.add("cm-mac");

const root = document.getElementById("app");
if (root) {
  createRoot(root).render(
    <ErrorBoundary>
      <ClaudeDeskDownApp />
    </ErrorBoundary>,
  );
}