import { Component, type ErrorInfo, type ReactNode } from "react";

interface Props {
  children: ReactNode;
  label?: string;
}

interface State { error: Error | null }

/** Keeps a failed preview/editor subtree from taking down the whole shell. */
export class EditorErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Qlisa editor preview failed", error, info.componentStack);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <div role="alert" style={fallbackStyle}>
        <span>{this.props.label ?? "Предпросмотр временно недоступен"}</span>
        <button type="button" onClick={() => this.setState({ error: null })} style={retryStyle}>Повторить</button>
      </div>
    );
  }
}

const fallbackStyle: React.CSSProperties = {
  display: "flex", alignItems: "center", justifyContent: "center", gap: 8,
  minHeight: 44, padding: 8, color: "var(--wc-text-muted)", fontSize: 11,
  border: "1px solid var(--wc-border)", borderRadius: 4, background: "var(--wc-bg-deepest)",
};

const retryStyle: React.CSSProperties = {
  border: "1px solid var(--wc-border-strong)", borderRadius: 3,
  color: "var(--wc-text)", background: "var(--wc-bg-surface)", cursor: "pointer", fontSize: 11,
};

