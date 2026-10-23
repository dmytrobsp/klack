# ttyak distribution: the accounts you set up

This page lists the accounts, keys and certificates that ttyak's releases need. Only a person can do these steps
(identity checks, payment, captchas). Claude does the rest: repositories, workflows, signing in CI, the site's domain
settings.

Prices and steps were checked on **2026-10-23**. Websites change. Where a step could not be confirmed, it says
**(verify)**: if what you see is different, do what the site says and tell Claude what was different.

## Safety rules for every step

- **Never paste a secret into a chat with Claude**, into an issue, or into a file inside the repository.
- Secrets go into GitHub in one of two ways:
  - In your own terminal: `gh secret set NAME -R dmytrobsp/ttyak`. It asks for the value and hides it. Paste the value
    and press Enter.
  - Or in the browser: the repository's **Settings → Secrets and variables → Actions → New repository secret**.
- Never use `gh secret set … --body VALUE`: that puts the secret into your shell history.
- To send a whole file (for example the Apple key file): `gh secret set NAME -R dmytrobsp/ttyak < path/to/file`.
- If GitHub says the repository `dmytrobsp/ttyak` is not found, the rename is not done yet. Wait until Claude says it is
  done. (Secrets stay with the repository when it is renamed.)
- To check what you have set: `gh secret list -R dmytrobsp/ttyak`. It shows names only, never values. Claude can run it
  too.
- When a step is done, tell Claude "the X secrets are set". Do not tell the values.

## Order and waiting times

Start the slow ones first. They wait for other people.

| Order | What | Your time | Waiting | Price (2026-10-23) |
|---|---|---|---|---|
| 1 | SSL.com certificate (Windows) | ~30 min | ID checks: 3–5 days ("standard validation") | $129/year + eSigner $180/year |
| 2 | Apple Developer Program (macOS) | ~20 min | review: often ~2 days, sometimes weeks (verify) | 99 USD/year |
| 3 | Domain `ttyak.sh` at Namecheap (done) | ~15 min | DNS: minutes to hours | your Namecheap order |
| 4 | AUR account (Arch Linux) | ~10 min | none | free |
| 5 | Chocolatey account (Windows) | ~10 min | email confirmation | free |
| 6 | GitHub: fork of winget-pkgs, two tokens | ~15 min | the token for the tap and bucket waits until Claude makes those repositories | free |
| later | Apple API key and certificate | ~20 min | after Apple approves you | — |
| later | SSL.com eSigner enrolment | ~15 min | after SSL.com validates you | — |

Total per year, roughly: $99 + $129 + $180 ≈ **$408**, plus the domain's renewal.

Start 1 and 2 today. 3 is done; 4 to 6 can be done any time, in any order.

## All the secrets

The release workflows use these GitHub secrets, by name only. A workflow job skips itself with a notice when its
secrets are missing.

| Secret name | What it is | Who sets it | Used for |
|---|---|---|---|
| `PUBLISH_TOKEN` | GitHub fine-grained token: Contents read/write on `homebrew-ttyak` and `scoop-ttyak` | you | Homebrew tap, Scoop bucket |
| `WINGET_TOKEN` | GitHub classic token with the `public_repo` scope | you | winget pull requests |
| `CHOCOLATEY_API_KEY` | Chocolatey API key | you | Chocolatey push |
| `AUR_SSH_KEY` | private SSH key for the AUR | Claude's script `tools/publish/aur-key.sh` | AUR push |
| `APPLE_P12` | Developer ID Application certificate and its key (p12, base64) | Claude's script `tools/signing/apple-csr.sh` | macOS signing |
| `APPLE_P12_PASSWORD` | the p12's password (random) | Claude's script `tools/signing/apple-csr.sh` | macOS signing |
| `APPLE_API_KEY` | the App Store Connect API key file (`AuthKey_….p8`), its whole text | you | macOS notarization |
| `APPLE_API_KEY_ID` | that key's Key ID | you | macOS notarization |
| `APPLE_API_ISSUER` | the Issuer ID | you | macOS notarization |
| `APPLE_TEAM_ID` | your Apple Team ID | you | macOS signing check |
| `ES_USERNAME` | your SSL.com account username | you | Windows signing |
| `ES_PASSWORD` | your SSL.com account password | you | Windows signing |
| `ES_CREDENTIAL_ID` | the eSigner credential ID of the certificate | you | Windows signing |
| `ES_TOTP_SECRET` | the eSigner TOTP "secret code" | you | Windows signing |

Claude's scripts never print a secret. They pipe it straight into `gh secret set`.

---

## 1. SSL.com: the Windows code-signing certificate

**What it is for.** Windows shows your legal name as the publisher of `ttyak.exe`. The certificate is an
"Individual Validation" (IV) code-signing certificate. Its key lives in SSL.com's cloud ("eSigner"), so CI can sign
without a USB token.

**Price (2026-10-23).** IV code signing: $129 for 1 year (longer terms are cheaper per year, down to $96.75/year for 5
years). eSigner "Tier 1": $15/month with 240 signings, which SSL.com's pricing guide explains as the yearly plan,
$180/year. Billed monthly it is $20/month with 20 signings a month (verify: from the 2024 guide). SSL.com's eSigner
page says: "New certificates will receive 30-days free of eSigner cloud signing." ttyak signs one file per release.
Expedited validation (+$599, 2 business days) is not needed.

**What you need.** A government photo ID that shows your full name, address, year of birth and photo. A camera for a
photo of you holding that ID next to your face (at least 5 megapixels). If your ID does not show your address, SSL.com
may ask for another document (verify).

**Steps (start today).**

1. Open the IV code-signing page: <https://www.ssl.com/products/software-integrity/code-signing/iv/>.
2. Choose **1 year** (or more). Where it asks how the key is stored, choose **eSigner Cloud Signing**, not a YubiKey
   (verify the exact label).
3. Pay, and create your SSL.com account. Use your **legal name**, exactly as on your ID. This name goes on the
   certificate and Windows shows it.
4. Prove who you are. Either:
   - the online check at <https://secure.ssl.com/users/iv_verification> (AI-based, after you log in), or
   - **Orders** → your order → **Documents** → **UPLOAD DOCUMENTS**: the ID (front, and back if it has one) and the
     photo of you holding it. Then **SUBMIT DOCUMENTS**.
5. Answer SSL.com's emails or phone call if they contact you (verify whether they call). Standard validation takes 3–5
   days.

**When SSL.com says you are validated: eSigner enrolment.**

6. Install an authenticator app on your phone if you have none (for example Google Authenticator or Authy).
7. Log in to SSL.com → **Orders** → your order → **details**.
8. In **eSigner Cloud Signing Enrollment**, under **Second factor authentication**, choose **OTP APP**.
9. Make a **4 digit PIN**. Save it in your password manager.
10. Click **create OTP and issue certificate**. A QR code appears, with a **secret code** next to it.
11. Scan the QR code with your authenticator app.
12. Set the TOTP secret now, in your terminal: `gh secret set ES_TOTP_SECRET -R dmytrobsp/ttyak`, and paste the
    **secret code**.
    - SSL.com says the QR code is shown only once. Other guides say you can see it (and the secret code) again: order
      → **download** → enter the PIN → **Show QR Code** (verify). Do not "reset" the QR code later: that may replace
      the secret, and CI signing would stop until you set the new one.
13. Find the **credential ID**: on the order's details page, in **SIGNING CREDENTIALS** (verify). It looks like a long
    id with dashes. Set it: `gh secret set ES_CREDENTIAL_ID -R dmytrobsp/ttyak`. If you cannot find it, tell Claude:
    CI can list it with the signer's `get_credential_ids` command.
14. Set your login: `gh secret set ES_USERNAME -R dmytrobsp/ttyak` (your SSL.com username, the one you log in with)
    and `gh secret set ES_PASSWORD -R dmytrobsp/ttyak` (your SSL.com password).
15. Buy the eSigner subscription (Tier 1, yearly) when SSL.com asks, before the 30 free days end (verify where it
    asks).

**Remember.** With these four values anyone can sign programs in your name. Keep them only in GitHub secrets, your
password manager and the authenticator app. If you change your SSL.com password, set `ES_PASSWORD` again.

**Claude then:** signs `ttyak.exe` in the release workflow with SSL.com's GitHub action `sslcom/esigner-codesign`.

---

## 2. Apple Developer Program: signing and notarizing for macOS

**What it is for.** macOS opens ttyak without a warning. Apple checks ("notarizes") each build. You enrol as an
**individual**; the certificate carries your legal name.

**Price (2026-10-23).** 99 USD per year (shown in your local currency). It renews every year until you cancel.

**What you need.** An Apple Account with two-factor authentication on
(<https://support.apple.com/en-us/102660>). Your **legal first and last name** in that Apple Account (no nickname: a
nickname delays the review). A real street address (no P.O. box). A photo ID (a passport is accepted in most
regions). A payment card in your name (a card in another person's name may cause a manual review).

**Steps (start today).**

1. Easiest: the **Apple Developer app** on an iPhone, iPad, or the Apple-silicon Mac. Use the **same device** from
   start to end. (The web, <https://developer.apple.com/programs/enroll/>, also has "Start your enrollment" (verify
   that it works for your region).)
2. Open the app → **Account** tab → sign in.
3. Tap **Enroll Now** → **Continue**.
4. Enter your legal first and last name and your phone number.
5. Take a photo of your photo ID when it asks.
6. Choose **Individual** as the entity type. Agree to the license agreement.
7. Under **Complete purchase**, tap **Subscribe** and pay.
8. Wait for Apple's email. Apple gives no official time. Reports say often about 2 days, sometimes weeks. If nothing
   happens after a week, contact Apple: <https://developer.apple.com/contact/>.

**When Apple has approved you: the API key (for notarization).**

9. Sign in to App Store Connect: <https://appstoreconnect.apple.com/> → **Users and Access** → **Integrations**.
10. If it shows **Request Access**: click it, tick the box, **Submit**. Apple approves it "case by case" (verify how
    long; often at once).
11. Click **Team Keys** → **Generate API Key** (or **+**) (verify the wording in App Store Connect).
12. Name: `ttyak notary`. Access: **Developer** (verify: reports say Developer is enough for notarization; if Claude's
    first run says "unauthorized", make a new key with **App Manager** and revoke the old one).
13. Click **Generate**. Then download the key file (the **Download** link in the key's row; verify the label). You can
    download it **only once**. The file is called `AuthKey_XXXXXXXXXX.p8`.
14. On the same page you see the **Issuer ID** (above the list) and the **Key ID** (in the key's row).
15. Set the four secrets in your terminal:
    - `gh secret set APPLE_API_KEY -R dmytrobsp/ttyak < ~/Downloads/AuthKey_XXXXXXXXXX.p8` (use your file's name)
    - `gh secret set APPLE_API_KEY_ID -R dmytrobsp/ttyak` (paste the Key ID)
    - `gh secret set APPLE_API_ISSUER -R dmytrobsp/ttyak` (paste the Issuer ID)
    - `gh secret set APPLE_TEAM_ID -R dmytrobsp/ttyak` (paste the Team ID: <https://developer.apple.com/account> →
      **Membership details** → **Team ID**, 10 characters)
16. Move the `.p8` file into your password manager, or delete it. If it is lost, revoke the key and make a new one.

**Then: the Developer ID Application certificate (with Claude).** Apple lets only the Account Holder (you) make this
certificate, on the website. Claude found no way to make it with the API key.

17. Tell Claude you are ready. Claude runs `tools/signing/apple-csr.sh request`. It makes a private key and a
    certificate request in `~/.config/ttyak-signing/` (your backup; only you can read it) and prints the request
    file's path.
18. Open <https://developer.apple.com/account/resources/certificates/list> → **+**.
19. Under **Software**, choose **Developer ID**, then **Developer ID Application** → **Continue**.
20. If it asks for a "Developer ID Certificate Intermediary", choose **G2 Sub-CA (Xcode 11.4.1 or later)**.
21. **Choose File** → the `.certSigningRequest` file Claude's script printed → **Continue** → **Download**.
22. Tell Claude the `.cer` file is in `~/Downloads` (usually `developerID_application.cer`). Claude runs
    `tools/signing/apple-csr.sh install` with it: the script checks the certificate, puts it into `APPLE_P12` and
    `APPLE_P12_PASSWORD`, and prints nothing secret. `tools/signing/apple-csr.sh status` shows the expiry date.

**Remember.** Certificates from Apple's G2 authority are **valid for one year**. Each year: steps 17–22 again (about 10
minutes), with `tools/signing/apple-csr.sh request --renew` in step 17 (it keeps the old files, dated). Builds signed
before then stay valid.

**Claude then:** signs and notarizes both macOS binaries in the release workflow.

---

## 3. Namecheap: the domain `ttyak.sh` (done)

**What it is for.** The site and the install one-liners live at `https://ttyak.sh` (for example
`curl -fsSL https://ttyak.sh/install.sh | sh`), served by GitHub Pages.

**What was done.** Porkbun refused the ID, so you bought `ttyak.sh` at **Namecheap** and set its DNS records there
by hand. No DNS script and no API key: nothing about the domain is kept on this machine.
No paid extras either (email, hosting, SSL): GitHub Pages gives the site a free certificate from Let's Encrypt.

The records (9), as GitHub's Pages docs give them for an apex domain plus `www`:

| Type | Host | Value |
|---|---|---|
| A | `@` | `185.199.108.153` |
| A | `@` | `185.199.109.153` |
| A | `@` | `185.199.110.153` |
| A | `@` | `185.199.111.153` |
| AAAA | `@` | `2606:50c0:8000::153` |
| AAAA | `@` | `2606:50c0:8001::153` |
| AAAA | `@` | `2606:50c0:8002::153` |
| AAAA | `@` | `2606:50c0:8003::153` |
| CNAME | `www` | `dmytrobsp.github.io.` |

Namecheap's parking records (what a new domain points to) were deleted, so only these answer.

**The verified domain.** On GitHub, your account's **Settings → Pages → Add a domain** → `ttyak.sh` gave a TXT record,
`_github-pages-challenge-dmytrobsp` with a value of its own, which you added at Namecheap too (a 10th record). Then
**Verify**. Keep the TXT record: while it is there, only your repositories can publish a Pages site on `ttyak.sh` and
its subdomains (no one can take the domain over if the site is ever switched off).

**Remember.** Keep auto-renew on for `ttyak.sh` at Namecheap (verify that it is on). If the domain lapses, the site, the
install one-liners and ttyak's update check (`https://ttyak.sh/latest.json`) stop working.

**Claude then:** once public DNS shows the records, sets `ttyak.sh` as the custom domain of the repository's Pages site
(through GitHub's API), waits for the certificate and turns on **Enforce HTTPS**. The site is deployed by the Pages
workflow (GitHub Actions), and for that GitHub ignores a `CNAME` file: the domain is a repository setting, so the repo
has no `site/CNAME`. GitHub then redirects `dmytrobsp.github.io/ttyak` and `www.ttyak.sh` to `https://ttyak.sh`.

---

## 4. AUR: the Arch Linux package `ttyak-bin`

**What it is for.** Once the AUR account exists, Arch users install with an AUR helper (`yay -S ttyak-bin`) and CI
pushes each new version over SSH. Until then the release's recipes zip carries the PKGBUILD (`makepkg -si`).

**Price.** Free.

**Steps.**

1. Open <https://aur.archlinux.org/register>.
2. Fill in **Username** and **Email Address**. You may tick **Hide Email Address**. Leave **SSH Public Key** empty for
   now.
3. The form shows a captcha: a command that starts with `LC_ALL=C pacman -V`. It needs `pacman`, which only Arch has.
   Easiest: paste the command to Claude and ask Claude to run it in a throwaway Arch container (the answer is not
   secret). Or run it yourself; this needs `podman` installed:
   `podman run --rm docker.io/library/archlinux:latest sh -c 'PASTE_THE_COMMAND_HERE'`. Type the 6 characters into
   **Answer**.
4. Click **Create**. The form has no password field: the AUR emails you a link to set your password.
5. Tell Claude your AUR account exists. Claude runs `tools/publish/aur-key.sh`: it makes a new SSH key, stores the
   private key straight into the secret `AUR_SSH_KEY`, and prints only the **public** key (one line starting with
   `ssh-ed25519`). The public key is not secret.
6. On the AUR: log in → **My Account** → paste the public key into **SSH Public Key** → type **Your current
   password** → **Update**.
7. Tell Claude it is done.

**Claude then:** makes the package, and the release workflow pushes it on each release. On 2026-10-23 the name
`ttyak-bin` was
free.

---

## 5. Chocolatey: the Windows package `ttyak`

**What it is for.** Windows users install with `choco install ttyak`. CI pushes each new version. Chocolatey reviews
every version before it is public (automatic checks start about 30 minutes after a push, then a person looks).

**Price.** Free.

**Steps.**

1. Open <https://community.chocolatey.org/> → **Sign Up** (verify the label), make an account, confirm the email.
2. Log in at <https://push.chocolatey.org/> and open <https://push.chocolatey.org/account>.
3. Copy the **API Key** shown there.
4. In your terminal: `gh secret set CHOCOLATEY_API_KEY -R dmytrobsp/ttyak`, and paste the key.
5. Tell Claude it is done.

**Claude then:** makes the package, and the release workflow pushes it on each release. Chocolatey sends review emails to you:
if a review asks for a change, forward the request (not any key) to Claude. On 2026-10-23 the id `ttyak` was free.

---

## 6. GitHub: winget fork and two tokens

**What it is for.** The release workflow pushes to two small repositories of yours (the Homebrew tap
`dmytrobsp/homebrew-ttyak` and the Scoop bucket `dmytrobsp/scoop-ttyak`) and opens a pull request to Microsoft's
winget list (`microsoft/winget-pkgs`) from your account.

**Price.** Free.

### 6a. Fork winget-pkgs (once)

The winget tool (Komac) sends its pull request from **your fork** of `microsoft/winget-pkgs`. It does not make the fork
itself.

1. Open <https://github.com/microsoft/winget-pkgs> → **Fork**.
2. Owner: your account. Name: `winget-pkgs` (keep it). **Create fork**.
3. On the first ttyak pull request, Microsoft's bot asks you to sign their Contributor License Agreement (once for all
   Microsoft repositories). Do what the bot's comment says: usually a reply `@microsoft-github-policy-service agree`
   (verify the wording in the bot's comment). Claude will tell you when that pull request exists.

### 6b. `WINGET_TOKEN`: a classic token

Komac needs a **classic** token. A fine-grained token cannot open the pull request to winget-pkgs.

1. GitHub → your picture → **Settings** → **Developer settings** → **Personal access tokens** → **Tokens (classic)** →
   **Generate new token** → **Generate new token (classic)**.
2. Note: `ttyak winget`. Expiration: **1 year** (put a reminder in your calendar to renew it).
3. Tick only **public_repo** (inside "repo").
4. **Generate token**. Copy it (GitHub shows it once).
5. In your terminal: `gh secret set WINGET_TOKEN -R dmytrobsp/ttyak`, and paste it.

### 6c. `PUBLISH_TOKEN`: a fine-grained token

**Wait** until Claude says the repositories `homebrew-ttyak` and `scoop-ttyak` exist: you can only pick repositories
that exist.

1. GitHub → **Settings** → **Developer settings** → **Personal access tokens** → **Fine-grained tokens** →
   **Generate new token**.
2. Token name: `ttyak publish`. Expiration: **1 year** (calendar reminder) or **No expiration**.
3. Resource owner: your account.
4. Repository access: **Only select repositories** → choose `homebrew-ttyak` and `scoop-ttyak`.
5. Permissions → Repository permissions → **Contents**: **Read and write**. (GitHub adds **Metadata: Read-only** by
   itself.) Nothing else.
6. **Generate token**. Copy it.
7. In your terminal: `gh secret set PUBLISH_TOKEN -R dmytrobsp/ttyak`, and paste it.
8. Tell Claude both tokens are set.

**When a token expires**, the publishing jobs fail with an error about authentication. Make a new token the same way
and set the secret again.

**Claude then:** creates the tap and bucket repositories (with your OK), and the release workflow jobs that push to them and to
winget.

---

## Sources

Checked on 2026-10-23.

- GitHub Pages' A and AAAA records for an apex domain, the `www` CNAME, and the redirects between them:
  <https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/managing-a-custom-domain-for-your-github-pages-site>
- "If you are publishing from a custom GitHub Actions workflow, any CNAME file is ignored and is not required":
  <https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/troubleshooting-custom-domains-and-github-pages>
- Verifying a domain (`_github-pages-challenge-<user>`, domain takeovers):
  <https://docs.github.com/en/pages/configuring-a-custom-domain-for-your-github-pages-site/verifying-your-custom-domain-for-github-pages>
- HTTPS for a custom domain (Let's Encrypt, Enforce HTTPS):
  <https://docs.github.com/en/pages/getting-started-with-github-pages/securing-your-github-pages-site-with-https>
- Apple enrolment requirements and fee: <https://developer.apple.com/programs/enroll/>
- Apple Developer app enrolment steps: <https://developer.apple.com/support/app-account/>
- Apple two-factor authentication: <https://support.apple.com/en-us/102660>
- App Store Connect API access and team keys: <https://developer.apple.com/help/app-store-connect/get-started/app-store-connect-api/>
- Developer ID certificates (Account Holder only, up to five):
  <https://developer.apple.com/help/account/certificates/create-developer-id-certificates/>
- G2 Sub-CA, one-year validity: <https://developer.apple.com/help/account/certificates/replace-developer-id-certificates>
- Notarization role (user reports, not official): <https://developer.apple.com/forums/thread/133063>
- SSL.com IV code signing price and validation time: <https://www.ssl.com/products/software-integrity/code-signing/iv/>
- SSL.com eSigner tiers: <https://www.ssl.com/products/software-integrity/signing-service/>,
  <https://www.ssl.com/guide/esigner-pricing/>
- SSL.com identity documents: <https://www.ssl.com/how-to/how-to-upload-validation-documents-for-ssl-com-certificates/>
- SSL.com eSigner enrolment: <https://www.ssl.com/how-to/enroll-esigner-remote-document-ev-code-signing/>
- SSL.com QR code and PIN: <https://www.ssl.com/how-to/view-reset-esigner-qr-code-reset-pin/>
- SSL.com TOTP secret for automation: <https://www.ssl.com/how-to/automate-esigner-ev-code-signing/>
- Viewing the QR code and secret code again (third-party): <https://infra.apache.org/code-signing-use.html>
- SSL.com GitHub action (current): <https://github.com/SSLcom/esigner-codesign>; the old `SSLcom/actions-codesigner`
  says it is deprecated: <https://github.com/SSLcom/actions-codesigner>
- AUR SSH keys and `ssh://aur@aur.archlinux.org/<pkgbase>.git`: <https://wiki.archlinux.org/title/AUR_submission_guidelines>
- AUR registration form and captcha (source code):
  <https://gitlab.archlinux.org/archlinux/aurweb/-/blob/master/templates/partials/account_form.html>,
  <https://gitlab.archlinux.org/archlinux/aurweb/-/blob/master/aurweb/captcha.py>
- Chocolatey API key location: <https://docs.chocolatey.org/en-us/create/commands/api-key>
- Chocolatey push: <https://docs.chocolatey.org/en-us/create/create-packages/>
- Chocolatey moderation: <https://docs.chocolatey.org/en-us/community-repository/moderation/>
- Komac (classic token with `public_repo`; uses your fork): <https://github.com/russellbanks/Komac>
- Microsoft CLA bot on winget-pkgs: <https://github.com/microsoft/winget-pkgs/pull/108450>
- GitHub personal access tokens:
  <https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens>
- `gh secret set`: <https://cli.github.com/manual/gh_secret_set>
