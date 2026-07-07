import { sessions } from "./auth";

export function StatusBadge({ userId }: { userId: string }) {
  const session = sessions.create(userId);
  return <span className="badge">session {session.id}</span>;
}
