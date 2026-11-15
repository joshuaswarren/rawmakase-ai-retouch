# Getting an API key

AI Retouch runs on a hosted AI service with your own account, so you need an
account and an API key with one of them: **fal.ai** (the default) or
**Replicate**. You only need one. Each request is one paid call to the
service: about $0.04 with the default models (November 2026; see the README's
[Services and models](../README.md#services-and-models), and the service's own
price page).

- [fal.ai](#falai)
- [Replicate](#replicate)
- [Putting the key into RAWmakase](#putting-the-key-into-rawmakase)
- [If something goes wrong](#if-something-goes-wrong)

## fal.ai

1. **Create an account.** Go to [fal.ai](https://fal.ai) and choose **Log
   in** or **Sign up**. You can sign in with GitHub, among other options.
2. **Add credit.** Open [Dashboard > Billing](https://fal.ai/dashboard/billing)
   and buy credit with a card. fal.ai sometimes gives new accounts some free
   credit, but don't count on it. When your balance runs out, fal.ai locks
   the account and rejects requests until you add more. Purchased credit
   expires a year after you buy it.
3. **Create a key.** Open [Dashboard > Keys](https://fal.ai/dashboard/keys),
   choose **Create Key** and give it a name such as `RAWmakase`. Choose the
   **API** preset: it allows calling models, and nothing more is needed.
4. **Copy the key right away.** fal.ai shows it only once. If you lose it,
   delete it and create a new one.

In RAWmakase, set **Service** to **fal.ai** and paste the key into **fal.ai
API key**.

## Replicate

1. **Create an account.** Go to [replicate.com](https://replicate.com) and
   choose **Sign in**. You can sign up with GitHub, among other options.
2. **Set up billing.** Open [Account > Billing](https://replicate.com/account/billing),
   add a payment method and buy credit with **Add credit**. Replicate needs
   billing set up before it runs most models, including `bria/eraser`. When
   your balance reaches zero, Replicate stops starting new work. Credit is
   valid for a year and isn't refundable.
3. **Get a token.** Open [Account > API tokens](https://replicate.com/account/api-tokens).
   Replicate makes a default token when you create the account; you can also
   create a new one named `RAWmakase`. A token is 40 characters and starts with
   `r8_`.

In RAWmakase, set **Service** to **Replicate** and paste the token into
**Replicate API token**. Leave **Replicate model** at `bria/eraser` unless you
want another model that takes an image and a mask.

## Putting the key into RAWmakase

1. Open **Preferences > Extensions** and choose **AI Retouch**.
2. Under **Settings**, pick the **Service** you signed up with.
3. Paste the key into its field and choose **Save**; the field then says the
   key is set. RAWmakase keeps it in your system's credential store (Keychain
   on macOS, Credential Manager on Windows, the Secret Service on Linux),
   never in its settings file, and never shows it again. To change it, paste
   a new one and Save; **Remove** deletes it.
4. Choose the **AI Retouch** tool (the magic wand on Develop's rail), set
   **Mode** to **AI Remove** or **AI Replace**, write what to do in **Prompt**
   (AI Replace needs it), paint over the area and send it with the drawer's
   button or Enter. The first time, RAWmakase asks whether it may
   send the area to the service: choose **Send**.

## If something goes wrong

- **"AI Remove needs setting up"** with no key set: no key is saved for the
  chosen Service, or it was saved in the other service's field.
- **"… didn't accept the API key"**: the key was mistyped, deleted on the
  service, or (fal.ai) created without the API preset. On fal.ai this is also
  what a locked account (no credit left) or a spending limit looks like, so
  check [Billing](https://fal.ai/dashboard/billing) too. Otherwise create a
  new key and save it.
- **"AI Remove is unavailable"** with a billing or balance message: add credit
  on the service's billing page.
- **The service declined this area**: the service refused the image's
  content. Nothing was changed.

Keep your key private: anyone with it can run models billed to your account.
If it leaks, delete it on the service and create a new one.
