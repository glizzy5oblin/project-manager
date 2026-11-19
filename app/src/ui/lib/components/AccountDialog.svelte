<script lang="ts">
    // The GitHub account: sign in with a device code, or reuse the GitHub CLI's login; see who
    // is signed in, the rate limit, and sign out. Closing the dialog mid sign-in cancels it.
    import { openUrl } from "@tauri-apps/plugin-opener";
    import { api, toAppError } from "../api";
    import type { Account } from "../bindings/Account";
    import type { AppError } from "../bindings/AppError";
    import type { AuthStatus } from "../bindings/AuthStatus";
    import type { DevicePrompt } from "../bindings/DevicePrompt";
    import { tasks } from "../tasks.svelte";
    import ErrorBox from "./ErrorBox.svelte";
    import Modal from "./Modal.svelte";

    let {
        onclose,
        onchange,
    }: {
        onclose: () => void;
        /** The signed-in account changed (null: signed out). */
        onchange: (account: Account | null) => void;
    } = $props();

    let status = $state<AuthStatus | null>(null);
    let busy = $state(false);
    let error = $state<AppError | null>(null);
    /** A device-flow sign-in in progress. */
    let pending = $state<{ taskId: number; prompt: DevicePrompt } | null>(null);
    let copied = $state(false);

    const account = $derived(status?.account ?? null);
    const gh = $derived(status?.gh ?? null);

    async function refresh() {
        busy = true;
        try {
            status = await api.githubStatus(true, true);
            onchange(status.account);
        } catch (e) {
            error = toAppError(e);
        } finally {
            busy = false;
        }
    }
    refresh();

    async function signIn() {
        error = null;
        copied = false;
        try {
            pending = await api.githubSignIn();
        } catch (e) {
            error = toAppError(e);
            return;
        }
        const outcome = await tasks.wait(pending.taskId);
        pending = null;
        if (outcome.status === "ok") {
            done(outcome.value as Account);
        } else if (outcome.status === "failed") {
            error = outcome.error;
        }
    }

    async function useGh() {
        error = null;
        busy = true;
        try {
            done(await api.githubUseGh());
        } catch (e) {
            error = toAppError(e);
        } finally {
            busy = false;
        }
    }

    function done(a: Account) {
        if (status) status = { ...status, account: a, notice: null };
        onchange(a);
        // Pick up the rate limit from the check that just ran.
        api.githubStatus(false, false).then(
            (s) => status && (status = { ...status, rateLimit: s.rateLimit }),
            () => {},
        );
    }

    async function signOut() {
        error = null;
        busy = true;
        try {
            await api.githubSignOut();
            onchange(null);
            await refresh();
        } catch (e) {
            error = toAppError(e);
        } finally {
            busy = false;
        }
    }

    async function copy(code: string) {
        try {
            await navigator.clipboard.writeText(code);
            copied = true;
        } catch {
            copied = false;
        }
    }

    async function open(url: string) {
        try {
            await openUrl(url);
        } catch (e) {
            error = toAppError(e);
        }
    }

    function close() {
        if (pending) api.taskCancel(pending.taskId);
        onclose();
    }

    function resetTime(unix: number): string {
        return new Date(unix * 1000).toLocaleTimeString([], {
            hour: "2-digit",
            minute: "2-digit",
        });
    }
</script>

<Modal title="GitHub account" onclose={close}>
    {#if status?.notice}
        <p class="notice" role="status">{status.notice}</p>
    {/if}

    {#if pending}
        <p>
            Enter this code on GitHub to let Project Manager publish releases
            for you:
        </p>
        <div class="code">
            <output aria-label="Sign-in code">{pending.prompt.userCode}</output>
            <button
                type="button"
                onclick={() => copy(pending!.prompt.userCode)}
            >
                {copied ? "Copied" : "Copy"}
            </button>
        </div>
        <p class="muted">
            Waiting for approval at <code>
                {pending.prompt.verificationUri}
            </code>. The code expires in {Math.round(
                pending.prompt.expiresIn / 60,
            )} minutes.
        </p>
    {:else if !status}
        <p class="muted">{busy ? "Checking…" : ""}</p>
    {:else if account}
        <dl>
            <dt>Signed in as</dt>
            <dd>
                <strong>{account.login}</strong>
                {#if account.name}<span class="muted">
                        ({account.name})
                    </span>{/if}
            </dd>
            <dt>Using</dt>
            <dd>
                {account.source === "app"
                    ? "Project Manager's sign-in, kept in the system keychain"
                    : "the GitHub CLI's login"}
            </dd>
            {#if account.scopes.length}
                <dt>Access</dt>
                <dd>
                    {#each account.scopes as s (s)}<code class="scope">
                            {s}
                        </code>{/each}
                </dd>
            {/if}
            {#if status.rateLimit}
                {@const r = status.rateLimit}
                <dt>API requests</dt>
                <dd>
                    {r.remaining.toLocaleString()} of {r.limit.toLocaleString()} left,
                    resets at {resetTime(r.reset)}
                </dd>
            {/if}
        </dl>
        <p class="muted">
            {account.source === "app"
                ? "Signing out removes the token from this computer. To revoke it on GitHub too, go to Settings → Applications → Authorized OAuth Apps."
                : "Signing out stops Project Manager using it; the GitHub CLI stays signed in."}
        </p>
    {:else}
        <p>
            Sign in to publish releases. Project Manager asks for access to
            public repositories only.
        </p>
        {#if !status.canSignIn}
            <p class="muted">
                This build has no GitHub sign-in set up. Use the GitHub CLI
                below, or see <code>docs/github.md</code>.
            </p>
        {/if}
        <div class="gh">
            {#if gh?.signedIn}
                <p>
                    The GitHub CLI {gh.version ? `(gh ${gh.version})` : ""} is signed
                    in. Project Manager can use its login instead.
                </p>
            {:else if gh?.installed}
                <p class="muted">
                    The GitHub CLI is installed but not signed in. Run
                    <code>gh auth login</code>, then check again.
                </p>
            {:else if gh}
                <p class="muted">
                    Or install the GitHub CLI (<code>gh</code>) and sign in
                    there.
                </p>
            {/if}
        </div>
    {/if}

    {#if error}<ErrorBox {error} />{/if}

    {#snippet actions()}
        {#if pending}
            <button type="button" onclick={close}>Cancel</button>
            <button
                type="button"
                class="primary"
                onclick={() => open(pending!.prompt.verificationUri)}
            >
                Open GitHub
            </button>
        {:else if account}
            <button type="button" onclick={close}>Close</button>
            <button type="button" onclick={signOut} disabled={busy}>
                Sign out
            </button>
        {:else}
            <button type="button" onclick={close}>Close</button>
            {#if status && !status.canSignIn && !gh?.signedIn}
                <button type="button" onclick={refresh} disabled={busy}>
                    Check again
                </button>
            {/if}
            {#if gh?.signedIn}
                <button
                    type="button"
                    class={status?.canSignIn ? "" : "primary"}
                    onclick={useGh}
                    disabled={busy}
                >
                    Use GitHub CLI login
                </button>
            {/if}
            {#if status?.canSignIn}
                <button
                    type="button"
                    class="primary"
                    onclick={signIn}
                    disabled={busy}
                >
                    Sign in with GitHub
                </button>
            {/if}
        {/if}
    {/snippet}
</Modal>

<style>
    p {
        margin: 0;
    }
    .muted {
        color: var(--text-muted);
    }
    .notice {
        padding: 0.5rem 0.75rem;
        border-left: 3px solid var(--warning);
        border-radius: 4px;
        background: var(--surface-2);
    }
    .code {
        display: flex;
        gap: 0.75rem;
        align-items: center;
        justify-content: center;
    }
    .code output {
        font-family: ui-monospace, monospace;
        font-size: 1.8rem;
        letter-spacing: 0.15em;
        user-select: all;
    }
    dl {
        display: grid;
        grid-template-columns: max-content minmax(0, 1fr);
        gap: 0.35rem 1rem;
        margin: 0;
    }
    dt {
        color: var(--text-muted);
    }
    dd {
        margin: 0;
        overflow-wrap: anywhere;
    }
    .scope {
        margin-right: 0.4rem;
    }
    code {
        overflow-wrap: anywhere;
    }
</style>
