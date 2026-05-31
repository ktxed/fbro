import { Component, type ReactNode } from "react";
import type { WsClient } from "../ws/client";

interface PanelErrorBoundaryProps {
  panelId: string;
  client: WsClient;
  resetKey: string;
  children: ReactNode;
}

interface PanelErrorBoundaryState {
  hasError: boolean;
  recoverRequested: boolean;
}

export class PanelErrorBoundary extends Component<PanelErrorBoundaryProps, PanelErrorBoundaryState> {
  state: PanelErrorBoundaryState = { hasError: false, recoverRequested: false };

  static getDerivedStateFromError(): Partial<PanelErrorBoundaryState> {
    return { hasError: true };
  }

  componentDidCatch(error: unknown) {
    console.error("Panel render failed:", error);
    if (!this.state.recoverRequested) {
      this.props.client.sendIntent("NavigateHome", this.props.panelId);
      this.setState({ recoverRequested: true });
    }
  }

  componentDidUpdate(prevProps: PanelErrorBoundaryProps) {
    if (prevProps.resetKey !== this.props.resetKey && this.state.hasError) {
      this.setState({ hasError: false, recoverRequested: false });
    }
  }

  render() {
    if (this.state.hasError) {
      return (
        <div className="panel panel--render-error" data-panel-id={this.props.panelId}>
          <div className="panel__render-error">
            Recovering panel…
          </div>
        </div>
      );
    }

    return this.props.children;
  }
}
