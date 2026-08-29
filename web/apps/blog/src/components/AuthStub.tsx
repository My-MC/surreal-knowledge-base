import styles from "./Stub.module.css";

type AuthStubProps = {
  mode: "login" | "register";
};

const LABELS: Record<AuthStubProps["mode"], { title: string; submit: string }> = {
  login: { title: "ログイン", submit: "ログイン" },
  register: { title: "アカウント登録", submit: "登録" },
};

/**
 * Skeleton only — todo 19 wires the real register/login flow over the
 * HttpOnly-cookie JWT endpoints. The form controls are inert placeholders.
 */
export function AuthStub({ mode }: AuthStubProps) {
  const label = LABELS[mode];
  return (
    <section className={styles.stub}>
      <h2 className={styles.title}>{label.title}</h2>
      <form className={styles.form}>
        <input type="email" placeholder="メールアドレス" aria-label="メールアドレス" disabled />
        <input type="password" placeholder="パスワード" aria-label="パスワード" disabled />
        <button type="submit" disabled>
          {label.submit}
        </button>
      </form>
      <p className={styles.note}>認証機能は準備中です。</p>
    </section>
  );
}
