# Privasi

Kantor adalah aplikasi desktop. Kantor tidak punya akun, analitik, atau telemetri, dan tidak
mengirim dokumen Anda ke mana pun. Kantor hanya terhubung ke jaringan dalam keadaan berikut:

1. **Pemeriksaan pembaruan.** Paling banyak sekali sehari, Kantor mengambil manifes kecil yang
   ditandatangani dari `https://kantor.paparanai.id/update/stable.json` (diteruskan ke rilis
   GitHub). Permintaan itu hanya membawa versi Kantor, sistem operasi, dan jenis CPU Anda di
   User-Agent (misalnya `Kantor/0.4.0 (windows; x86_64)`). Tidak ada pengenal, data penggunaan,
   atau apa pun tentang berkas Anda. Jika ada pembaruan, Kantor mengunduhnya dari GitHub.
   Matikan dengan `[update] enabled = false` di `settings.toml`; setelah itu Kantor sama sekali
   tidak memeriksa pembaruan. Rinciannya: [README, Updates](README.md#updates).
2. **Fitur AI, hanya jika Anda menyalakannya.** AI mati secara bawaan. Jika Anda memilih penyedia
   (Paparan AI, Anthropic, OpenAI, Google dan lainnya, atau Ollama di komputer Anda sendiri) lalu
   menjalankan perintah AI, Kantor hanya mengirim apa yang dibutuhkan perintah itu ke penyedia
   pilihan Anda, menurut ketentuan penyedia tersebut. Rinciannya:
   [README, AI features and privacy](README.md#ai-features-optional-and-privacy).
3. **Rumus yang mengunduh, di Sheets.** Rumus `IMAGE` dan `WEBSERVICE` di buku kerja menyebut
   alamat di internet, dan Kantor mengambilnya seperti Excel: gambar yang ditampilkan `IMAGE`,
   teks yang dikembalikan `WEBSERVICE`. Jadi membuka buku kerja bisa menghubungi server yang
   dipilih pembuatnya; server itu melihat alamat IP Anda, waktu, dan User-Agent, dan alamatnya
   sendiri bisa membawa data dari buku kerja. Buku kerja yang dibuka dari berkas bertanya dulu,
   lewat bilah di atas kisi ("Izinkan / Blokir"); sebelum Anda mengizinkan, tidak ada yang
   diunduh. Unduhan hanya disimpan di memori sampai Kantor ditutup. Matikan di mana saja dengan
   `[sheets] web_formulas = false` di `settings.toml` atau lewat palet perintah (`Ctrl K`);
   rumusnya lalu menampilkan `#BLOCKED!`. `kantor convert` tidak pernah mengunduh.
4. **Memeriksa tanda tangan PDF secara daring, hanya saat Anda mengekliknya.** PDF bertanda tangan
   diperiksa dengan isi berkasnya. Jika penerbit sertifikat atau status pencabutannya tidak ada,
   panel Tanda Tangan menawarkan *Periksa Daring*; hanya saat itu Kantor mengunduh sertifikat
   penerbit yang kurang dan bertanya ke otoritas sertifikat (OCSP dan daftar pencabutan) apakah
   sertifikatnya dicabut. Server itu melihat alamat IP Anda, waktu, User-Agent, dan sertifikat
   mana yang ditanyakan, bukan dokumennya. Matikan dengan `[pdf] online_signature_checks = false`
   di `settings.toml`. `kantor pdf verify` hanya daring dengan `--online`.
5. **Penyimpanan awan, hanya yang Anda tambahkan.** Berkas yang Anda buka dari akun WebDAV dikirim
   ke dan dari server itu.
6. **Google Drive dan Microsoft OneDrive, hanya jika Anda masuk.** Saat Anda menambahkan salah satu
   akun ini di *Akun Awan*, Anda masuk lewat peramban Anda sendiri, dengan Google atau Microsoft,
   bukan dengan Kantor. Setelah itu Kantor berbicara langsung dengan Google atau Microsoft dari
   komputer Anda. Tidak ada server Kantor di antaranya, dan tidak ada yang dikirim ke proyek Kantor.
   - **Apa yang bisa dijangkau Kantor.** Di Google Drive, Kantor hanya meminta izin `drive.file`:
     berkas dan folder yang dibuat Kantor atau yang Anda buka dengan Kantor, bukan isi Drive Anda
     yang lain. Di OneDrive, Kantor meminta `Files.ReadWrite`: OneDrive milik Anda sendiri. Pada
     keduanya Kantor juga mendapat nama dan alamat email Anda, yang ditampilkan di *Akun Awan*
     agar Anda bisa membedakan akun.
   - **Apa yang dilakukan dengannya.** Menampilkan folder, membuka berkas yang Anda pilih,
     menyimpan suntingan Anda kembali (jika simpan otomatis awan menyala, beberapa detik setelah
     Anda berhenti mengetik; matikan dengan `[cloud] autosave = false` di `settings.toml`), dan
     membuat berkas serta folder yang Anda minta. Tidak lebih: Kantor tidak membaca berkas lain,
     tidak menyimpan salinan di mana pun selain komputer Anda, tidak menampilkan iklan, tidak
     menjual atau membagikan datanya, dan tidak memakainya untuk melatih model AI.
   - **Fitur AI.** Jika Anda menyalakan penyedia AI dan menjalankan perintah AI pada dokumen yang
     dibuka dari Drive atau OneDrive, perintah itu mengirim apa yang dibutuhkannya ke penyedia
     pilihan Anda, seperti di poin 2. Tidak ada yang dikirim ke penyedia AI kecuali Anda
     menjalankan perintah seperti itu.
   - **Token masuk.** Google atau Microsoft memberi Kantor token agar Anda tetap masuk. Token itu
     disimpan di gantungan kunci (keychain) sistem Anda (atau, jika tidak ada, di berkas yang
     hanya bisa dibaca akun pengguna Anda) dan hanya dikirim kembali ke Google atau Microsoft.
   - **Mencabut akses.** Menghapus akun di *Akun Awan* menghapus token dari komputer Anda dan,
     untuk Google, mencabutnya. Anda juga bisa mencabut akses Kantor kapan saja di
     <https://myaccount.google.com/permissions> atau <https://account.live.com/consent/Manage>
     (akun kantor dan sekolah: <https://myapps.microsoft.com>).
   - Penggunaan dan pemindahan informasi yang diterima Kantor dari API Google mematuhi
     [Kebijakan Data Pengguna Layanan API Google](https://developers.google.com/terms/api-services-user-data-policy),
     termasuk persyaratan Penggunaan Terbatas (Limited Use).

Server pembaruan (Cloudflare Pages dan GitHub) mungkin menyimpan log permintaan standar (alamat IP,
waktu, User-Agent) menurut kebijakan mereka sendiri. Proyek Kantor tidak mengumpulkan atau
menyimpannya.

Semua yang lain tetap di komputer Anda: pengaturan, salinan pemulihan pekerjaan yang belum disimpan,
dan koneksi MCP lokal yang bisa dipakai asisten AI di komputer Anda, yang hanya menerima koneksi dari
akun pengguna Anda sendiri.

Pertanyaan: buka issue di https://github.com/dzulfikar08/kantor/issues.
