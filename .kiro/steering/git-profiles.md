# Git Multi-Profile Push Rules

Before every commit and push, you MUST follow these rules without being asked.

## The 4 GitHub Profiles

| Alias | user.name | user.email | SSH Host Alias |
|---|---|---|---|
| jeremiah / jeremiahniffypeter | Jeremiah Peters | jeremiahniffypeter@gmail.com | github-global |
| david / devoclan | devoclan | jeremiahniffpeter02@gmail.com | github-david |
| josunday | josunday002 | josunday002@gmail.com | github-joe |
| martins | martinzhames | martinzhames02@gmail.com | github-martins |

## Required Steps Before Every Commit + Push

1. Run `git remote -v` to check the current remote URL
2. Set the remote URL to match the target profile's SSH alias
3. Set `git config user.name` for that profile
4. Set `git config user.email` for that profile
5. Then commit and push

## Remote URL Format

```
git remote set-url origin git@<ssh-alias>:<github-username>/<repo>.git
```

## Per-Profile Commands

### Jeremiah (github-global)
```bash
git remote set-url origin git@github-global:jeremiahniffypeter/<repo>.git
git config user.name "Jeremiah Peters"
git config user.email "jeremiahniffypeter@gmail.com"
```

### David (github-david)
```bash
git remote set-url origin git@github-david:devoclan/<repo>.git
git config user.name "devoclan"
git config user.email "jeremiahniffpeter02@gmail.com"
```

### Josunday (github-joe)
```bash
git remote set-url origin git@github-joe:josunday002/<repo>.git
git config user.name "josunday002"
git config user.email "josunday002@gmail.com"
```

### Martins (github-martins)
```bash
git remote set-url origin git@github-martins:martinzhames/<repo>.git
git config user.name "martinzhames"
git config user.email "martinzhames02@gmail.com"
```

## Key Rules

- NEVER push without first setting the correct remote URL and git config for the target profile
- Git config user.name/email only labels the commit — the SSH alias in the remote URL is what determines which GitHub account receives the push
- Always infer the repo name from the current directory if not explicitly provided
- If the user says "push to martins", "push to david", "push to josunday", or "push to jeremiah" — apply the matching profile above automatically
