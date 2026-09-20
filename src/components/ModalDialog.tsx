import { forwardRef, type HTMLAttributes } from "react";

interface ModalDialogProps extends HTMLAttributes<HTMLDivElement> {
  ariaLabel: string;
  ariaModal?: boolean;
  role?: "dialog" | "alertdialog";
  testId?: string;
}

export const ModalDialog = forwardRef<HTMLDivElement, ModalDialogProps>(function ModalDialog(
  {
    ariaLabel,
    ariaModal = true,
    role = "dialog",
    testId,
    className = "",
    children,
    ...props
  },
  ref,
) {
  return (
    <div
      {...props}
      ref={ref}
      className={`modal ${className}`.trim()}
      role={role}
      aria-modal={ariaModal}
      aria-label={ariaLabel}
      data-testid={testId}
    >
      {children}
    </div>
  );
});
