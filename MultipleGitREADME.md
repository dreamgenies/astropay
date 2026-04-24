# Multiple GitHub Accounts — Complete Setup & Usage Guide

This document contains everything needed for any LLM (or human) to understand, set up, and operate a multi-GitHub-account workflow on this machine. Read every section before taking any action.

---

## 1. The Core Concept (Read This First)

Git does NOT choose which GitHub account to push to based on `user.name` or `user.email`. Those fields only label the commit author — they are cosmetic.

The actual GitHub account used when pushing is determined by:
1. The **remote URL** — specifically the SSH host alias in it
2. The **SSH config** — which maps that alias to a specific private key
3. The **SSH key** — which is registered to a specific GitHub account

So the correct mental model is:

```
Remote URL  →  selects which SSH alias to use
SSH alias   →  maps to a specific private key (~/.ssh/config)
Private key →  is registered on a specific GitHub account
GitHub      →  authenticates and accepts the push as that account
git config  →  only labels who authored the commit (name/email)
```

This means:
- Changing `git config user.name` alone does NOT switch accounts
- You MUST change the remote URL to the correct SSH alias to push to the right account
- You MUST also set `user.name` and `user.email` so commits are labeled correctly

---

## 2. The 4 Profiles on This Machine

| Profile Alias | Git user.name | Git user.email | SSH Host Alias | GitHub Username |
|---|---|---|---|---|
| jeremiah / jeremiahniffypeter | Jeremiah Peters | jeremiahniffypeter@gmail.com | github-global | jeremiahniffypeter |
| david / devoclan | devoclan | jeremiahniffpeter02@gmail.com | github-david | devoclan |
| josunday | josunday002 | josunday002@gmail.com | github-joe | josunday002 |
| martins | martinzhames | martinzhames02@gmail.com | github-martins | martinzhames |

---

## 3. SSH Keys on This Machine

Each profile has its own SSH key pair stored in `~/.ssh/`:

| Profile | Private Key File | Public Key File |
|---|---|---|
| jeremiah | ~/.ssh/id_ed25519_global | ~/.ssh/id_ed25519_global.pub |
| david | ~/.ssh/id_ed25519_david | ~/.ssh/id_ed25519_david.pub |
| josunday | ~/.ssh/id_ed25519_joe | ~/.ssh/id_ed25519_joe.pub |
| martins | ~/.ssh/id_ed25519_martins | ~/.ssh/id_ed25519_martins.pub |

Each public key is registered on its corresponding GitHub account under:
`Settings → SSH and GPG Keys`

---

## 4. SSH Config (~/.ssh/config)

This file maps each SSH host alias to the correct private key. It must contain all four entries:

```
# Jeremiah (Global / Default)
Host github-global
  HostName github.com
  User git
  IdentityFile ~/.ssh/id_ed25519_global

# David
Host github-david
  HostName github.com
  User git
  IdentityFile ~/.ssh/id_ed25519_david

# Josunday
Host github-joe
  HostName github.com
  User git
  IdentityFile ~/.ssh/id_ed25519_joe

# Martins
Host github-martins
  HostName github.com
  User git
  IdentityFile ~/.ssh/id_ed25519_martins
```

To view or edit: `nano ~/.ssh/config`

---

## 5. How to Test Each SSH Connection

Run these to confirm each key is correctly linked to the right GitHub account:

```bash
ssh -T git@github-global
ssh -T git@github-david
ssh -T git@github-joe
ssh -T git@github-martins
```

Expected output for each:
```
Hi <username>! You've successfully authenticated, but GitHub does not provide shell access.
```

Each one should greet the correct GitHub username for that profile.

---

## 6. Remote URL Format

The remote URL must use the SSH host alias, NOT `github.com` directly.

**Wrong (uses wrong or default account):**
```
git@github.com:username/repo.git
```

**Correct (uses specific profile):**
```
git@github-martins:martinzhames/repo.git
git@github-david:devoclan/repo.git
git@github-joe:josunday002/repo.git
git@github-global:jeremiahniffypeter/repo.git
```

---

## 7. Per-Profile Remote URL + Git Config Commands

Use these exact commands before committing and pushing to each profile.

### Jeremiah
```bash
git remote set-url origin git@github-global:jeremiahniffypeter/<repo>.git
git config user.name "Jeremiah Peters"
git config user.email "jeremiahniffypeter@gmail.com"
```

### David
```bash
git remote set-url origin git@github-david:devoclan/<repo>.git
git config user.name "devoclan"
git config user.email "jeremiahniffpeter02@gmail.com"
```

### Josunday
```bash
git remote set-url origin git@github-joe:josunday002/<repo>.git
git config user.name "josunday002"
git config user.email "josunday002@gmail.com"
```

### Martins
```bash
git remote set-url origin git@github-martins:martinzhames/<repo>.git
git config user.name "martinzhames"
git config user.email "martinzhames02@gmail.com"
```

Replace `<repo>` with the actual repository name (infer from the current directory if not stated).

---

## 8. The Mandatory Pre-Push Checklist

Before EVERY commit and push, an LLM or operator MUST do the following steps in order:

1. **Check current remote:**
   ```bash
   git remote -v
   ```

2. **Set remote URL** to the target profile's SSH alias (see Section 7)

3. **Set git config user.name** for the target profile (see Section 7)

4. **Set git config user.email** for the target profile (see Section 7)

5. **Stage changes:**
   ```bash
   git add -A
   ```

6. **Commit:**
   ```bash
   git commit -m "your message"
   ```

7. **Push:**
   ```bash
   git push -u origin <branch-name>
   ```

Never skip steps 2, 3, or 4. Never push using `github.com` in the remote URL.

---

## 9. Cloning a Repo for a Specific Profile

When cloning a new repo, use the SSH alias in the URL from the start:

```bash
# Jeremiah
git clone git@github-global:jeremiahniffypeter/<repo>.git

# David
git clone git@github-david:devoclan/<repo>.git

# Josunday
git clone git@github-joe:josunday002/<repo>.git

# Martins
git clone git@github-martins:martinzhames/<repo>.git
```

Then immediately set the local git config:
```bash
cd <repo>
git config user.name "<name>"
git config user.email "<email>"
```

---

## 10. Fixing the Remote on an Existing Repo

If a repo was cloned with the wrong URL or needs to switch profiles:

```bash
# Check what it currently is
git remote -v

# Update it
git remote set-url origin git@github-<alias>:<username>/<repo>.git
```

---

## 11. Full Workflow Example — Stash, Branch, Pop, Commit, Push

This is the exact workflow used on this machine when work is in progress and needs to be moved to a new branch and pushed to a specific profile.

```bash
# 1. Stash all current changes including untracked files
git stash -u -m "work in progress"

# 2. Create and switch to a new branch (choose a descriptive name)
git checkout -b feat/<descriptive-branch-name>

# 3. Restore stashed changes onto the new branch
git stash pop

# 4. Set remote URL for the target profile (example: martins)
git remote set-url origin git@github-martins:martinzhames/<repo>.git

# 5. Set git identity for the target profile
git config user.name "martinzhames"
git config user.email "martinzhames02@gmail.com"

# 6. Stage, commit, and push
git add -A
git commit -m "your descriptive commit message"
git push -u origin feat/<descriptive-branch-name>
```

---

## 12. How to Verify Everything Is Correct Before Pushing

```bash
# Confirm remote URL has the right SSH alias
git remote -v

# Confirm git identity is set correctly
git config user.name
git config user.email

# Confirm you are on the right branch
git branch

# Test SSH connection for the target profile
ssh -T git@github-martins   # or whichever alias
```

---

## 13. Setting Up From Scratch (If Keys Are Lost or New Machine)

### Step 1 — Generate SSH Keys

```bash
ssh-keygen -t ed25519 -C "jeremiahniffypeter@gmail.com" -f ~/.ssh/id_ed25519_global
ssh-keygen -t ed25519 -C "jeremiahniffpeter02@gmail.com" -f ~/.ssh/id_ed25519_david
ssh-keygen -t ed25519 -C "josunday002@gmail.com" -f ~/.ssh/id_ed25519_joe
ssh-keygen -t ed25519 -C "martinzhames02@gmail.com" -f ~/.ssh/id_ed25519_martins
```

When prompted for a passphrase, set one for security or leave empty.

### Step 2 — Add Keys to SSH Agent

```bash
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519_global
ssh-add ~/.ssh/id_ed25519_david
ssh-add ~/.ssh/id_ed25519_joe
ssh-add ~/.ssh/id_ed25519_martins
```

### Step 3 — Copy Each Public Key

```bash
cat ~/.ssh/id_ed25519_global.pub
cat ~/.ssh/id_ed25519_david.pub
cat ~/.ssh/id_ed25519_joe.pub
cat ~/.ssh/id_ed25519_martins.pub
```

### Step 4 — Add Each Public Key to the Correct GitHub Account

For each profile:
1. Log into that GitHub account
2. Go to: `Settings → SSH and GPG Keys → New SSH Key`
3. Paste the public key
4. Name it something like `laptop-martins`, `laptop-david`, etc.

### Step 5 — Write the SSH Config

```bash
nano ~/.ssh/config
```

Paste the full config from Section 4 above. Save and exit.

### Step 6 — Test All Connections

```bash
ssh -T git@github-global
ssh -T git@github-david
ssh -T git@github-joe
ssh -T git@github-martins
```

Each should respond with `Hi <username>! You've successfully authenticated...`

---

## 14. Common Mistakes to Avoid

| Mistake | Why It's Wrong |
|---|---|
| Using `git@github.com:...` in remote URL | Uses the default SSH key, likely the wrong account |
| Only changing `git config user.name/email` | Does NOT switch GitHub accounts, only labels the commit |
| Forgetting to run `git remote set-url` | Push goes to the wrong GitHub account |
| Not testing with `ssh -T` after setup | You won't know if the key is linked correctly |
| Using `git config --global user.name` | Changes identity globally, breaks other repos |
| Cloning with `github.com` instead of alias | Repo is linked to wrong account from the start |

---

## 15. Instructions for Any LLM Given This Document

If you are an LLM reading this document, follow these rules for every commit and push operation:

1. The user will tell you which profile to push to by name (e.g. "push to martins", "push to david", "push to josunday", "push to jeremiah")
2. Look up that profile in Section 2 of this document
3. Run `git remote -v` first to check the current remote
4. Run `git remote set-url origin git@github-<alias>:<github-username>/<repo>.git` using the correct alias and username from Section 2
5. Run `git config user.name "<name>"` using the exact name from Section 2
6. Run `git config user.email "<email>"` using the exact email from Section 2
7. Infer the repo name from the current working directory if the user has not stated it
8. Then stage, commit with a descriptive message, and push
9. Never skip the remote URL step — it is the most critical step
10. Never use `github.com` directly in the remote URL — always use the SSH alias

---

## 16. Quick Reference Card

```
PROFILE     | SSH ALIAS       | user.name        | user.email
------------|-----------------|------------------|---------------------------
jeremiah    | github-global   | Jeremiah Peters  | jeremiahniffypeter@gmail.com
david       | github-david    | devoclan         | jeremiahniffpeter02@gmail.com
josunday    | github-joe      | josunday002      | josunday002@gmail.com
martins     | github-martins  | martinzhames     | martinzhames02@gmail.com

REMOTE URL FORMAT:
  git@<SSH-ALIAS>:<github-username>/<repo>.git

EXAMPLES:
  git@github-martins:martinzhames/astropay.git
  git@github-david:devoclan/myproject.git
  git@github-joe:josunday002/myrepo.git
  git@github-global:jeremiahniffypeter/myrepo.git
```
