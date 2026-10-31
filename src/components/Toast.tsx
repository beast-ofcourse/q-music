import { Icon } from "./Icon";
import { useStore } from "../state/store";

export function Toast() {
  const { toast, dismissToast } = useStore();
  if (!toast) return null;

  return (
    <div className="toast" role="status" data-tone={toast.tone}>
      <span>{toast.message}</span>
      <button
        type="button"
        className="toast-close"
        aria-label="Dismiss"
        onClick={dismissToast}
      >
        <Icon name="close" size={12} />
      </button>
    </div>
  );
}
