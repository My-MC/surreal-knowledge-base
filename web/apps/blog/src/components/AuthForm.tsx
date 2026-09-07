import { Link, useNavigate } from "@tanstack/react-router";
import { type FormEvent, useState } from "react";
import { ApiError, loginQuery, registerQuery } from "../api";
import { useAuthStore } from "../auth";
import styles from "./Form.module.css";

type AuthFormProps = {
  mode: "login" | "register";
};

const LABELS: Record<AuthFormProps["mode"], { title: string; submit: string }> = {
  login: { title: "ログイン", submit: "ログイン" },
  register: { title: "アカウント登録", submit: "登録" },
};

/**
 * /login and /register. Register auto-logins: the register response sets no
 * cookie, so the login call runs right after a 201 and the user lands on "/"
 * already signed in. The server decides the role — registration is
 * reader-only unless the optional 招待トークン matches the operator's
 * SKB_SERVER_AUTHOR_INVITES entry for the email; errors (409 duplicate
 * email, 400 validation, 401 bad credentials) render inline from the server
 * message.
 */
export function AuthForm({ mode }: AuthFormProps) {
  const label = LABELS[mode];
  const navigate = useNavigate();
  const setAuth = useAuthStore((state) => state.setAuth);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [invite, setInvite] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: FormEvent) {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      if (mode === "register") {
        const trimmedInvite = invite.trim();
        await registerQuery(email, password, trimmedInvite === "" ? undefined : trimmedInvite);
      }
      const auth = await loginQuery(email, password);
      setAuth(auth.email, auth.role);
      await navigate({ to: "/" });
    } catch (err) {
      setError(err instanceof ApiError ? err.message : "予期しないエラーが発生しました。");
    } finally {
      setPending(false);
    }
  }

  return (
    <section className={styles.stub}>
      <h2 className={styles.title}>{label.title}</h2>
      <form className={styles.form} onSubmit={handleSubmit}>
        <input
          type="email"
          placeholder="メールアドレス"
          aria-label="メールアドレス"
          data-testid="auth-email"
          value={email}
          onChange={(event) => setEmail(event.target.value)}
          required
        />
        <input
          type="password"
          placeholder="パスワード"
          aria-label="パスワード"
          data-testid="auth-password"
          value={password}
          onChange={(event) => setPassword(event.target.value)}
          required
        />
        {mode === "register" && (
          <input
            type="text"
            placeholder="招待トークン（author 招待を受けた場合のみ）"
            aria-label="招待トークン"
            data-testid="auth-invite"
            value={invite}
            onChange={(event) => setInvite(event.target.value)}
            autoComplete="off"
          />
        )}
        <button type="submit" data-testid="auth-submit" disabled={pending}>
          {label.submit}
        </button>
        {error !== null && (
          <p className={styles.error} role="alert" data-testid="auth-error">
            {error}
          </p>
        )}
      </form>
      <p className={styles.note}>
        {mode === "login" ? (
          <>
            アカウントをお持ちでない方は <Link to="/register">新規登録</Link> へ。
          </>
        ) : (
          <>
            既にアカウントをお持ちの方は <Link to="/login">ログイン</Link> へ。
          </>
        )}
      </p>
    </section>
  );
}
