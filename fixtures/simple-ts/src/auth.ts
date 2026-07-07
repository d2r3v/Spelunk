import { hashPassword, verifyPassword } from "./util";

const SESSION_TTL_MS = 1000 * 60 * 30;

export interface Credentials {
  username: string;
  password: string;
}

export interface Session {
  id: string;
  userId: string;
  expiresAt: number;
}

interface User {
  id: string;
  username: string;
  passwordHash: string;
}

const users = new Map<string, User>();

export async function login(creds: Credentials): Promise<Session | null> {
  const user = users.get(creds.username);
  if (!user) {
    return null;
  }
  if (!verifyPassword(creds.password, user.passwordHash)) {
    return null;
  }
  return sessions.create(user.id);
}

export async function register(creds: Credentials): Promise<User> {
  const user: User = {
    id: crypto.randomUUID(),
    username: creds.username,
    passwordHash: hashPassword(creds.password),
  };
  users.set(user.username, user);
  return user;
}

export class SessionManager {
  private active = new Map<string, Session>();

  create(userId: string): Session {
    const session: Session = {
      id: crypto.randomUUID(),
      userId,
      expiresAt: Date.now() + SESSION_TTL_MS,
    };
    this.active.set(session.id, session);
    return session;
  }

  refresh(sessionId: string): Session | null {
    const session = this.active.get(sessionId);
    if (!session || session.expiresAt < Date.now()) {
      return null;
    }
    session.expiresAt = Date.now() + SESSION_TTL_MS;
    return session;
  }

  revoke(sessionId: string): void {
    this.active.delete(sessionId);
  }
}

export const sessions = new SessionManager();
