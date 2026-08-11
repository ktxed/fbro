export interface GitActivity {
  id: string;
  action: string;
  message: string;
  status: "running" | "complete" | "failed";
}
