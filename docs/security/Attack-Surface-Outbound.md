# Attack surface: outbound requests

Requests the server itself makes. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## Printing, SFTP and push

IPP printing and the SFTP client are outbound. Git, DAV, `/termws`, and `/notws` are routes on the public HTTP server. Web Push (the `web-push` feature) posts to the push service named by each subscription a browser stored, with a VAPID token; internal addresses are refused, no redirect is followed, and the payload is encrypted to the browser and holds no message body. FCM (`service/fcm.rs`) posts only to the token endpoint of the configured service account (`fcm_service_account`, is-secret, never generated) and to `fcm_base_url` (FCM by default), both from the conf, through the framework's outbound HTTP; the data message holds what a Web Push holds, plus a ring's call id, room id, caller name, video flag and expiry (and a ring's end, its call id and reason), and no message body.
