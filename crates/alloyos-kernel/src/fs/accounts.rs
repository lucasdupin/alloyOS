//! OS-only credential store. Raw records never pass through ToolVfsBackend or
//! app/script hosts. CobaltFS durability is not encryption or a hardware fsync.
use super::vfs::{FsError, InodeKind, Vfs, VFS};
use alloc::{
    format,
    string::String,
    sync::Arc,
    vec::Vec,
};
use alloyos_mcp::account::{AccountError as Error, SecretStorage};
use alloyos_welder::oauth::{ClientProfile, CredentialStore, PrivateAccount};
use spin::Mutex;
const ROOT: &str = "/system/accounts";
const LIMIT: usize = 32768;
#[derive(Default)]
pub struct KernelCredentialStore {
    instance: Option<Arc<Mutex<Vfs>>>,
    #[cfg(test)]
    fail_at: Option<usize>,
    #[cfg(test)]
    mutations: usize,
}
impl core::fmt::Debug for KernelCredentialStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("KernelCredentialStore([private])")
    }
}
impl KernelCredentialStore {
    pub fn global() -> Self {
        Self::default()
    }
    pub fn from_vfs(vfs: Arc<Mutex<Vfs>>) -> Self {
        Self {
            instance: Some(vfs),
            ..Self::default()
        }
    }
    fn with_vfs<T>(&self, f: impl FnOnce(&Vfs) -> Result<T, Error>) -> Result<T, Error> {
        match &self.instance {
            Some(vfs) => f(&vfs.lock()),
            None => {
                let guard = VFS.lock();
                f(guard.as_ref().ok_or(Error::StorageUnavailable)?)
            }
        }
    }
    fn mutation(&mut self) -> Result<(), Error> {
        #[cfg(test)]
        {
            self.mutations += 1;
            if self.fail_at == Some(self.mutations) {
                return Err(Error::StorageUnavailable);
            }
        }
        Ok(())
    }
    fn ensure_dirs(&mut self) -> Result<(), Error> {
        for path in [
            "/system",
            ROOT,
            "/system/accounts/records",
            "/system/accounts/profiles",
        ] {
            let exists = self.with_vfs(|v| match v.root_fs.lookup(path) {
                Ok(inode) if inode.kind == InodeKind::Directory => Ok(true),
                Err(FsError::NotFound) => Ok(false),
                _ => Err(Error::StorageUnavailable),
            })?;
            if !exists {
                self.mutation()?;
                self.with_vfs(|v| {
                    v.root_fs
                        .create(path, InodeKind::Directory)
                        .map(|_| ())
                        .map_err(|_| Error::StorageUnavailable)
                })?;
            }
        }
        Ok(())
    }
    fn exists(&self, path: &str) -> Result<bool, Error> {
        self.with_vfs(|v| match v.root_fs.lookup(path) {
            Ok(_) => Ok(true),
            Err(FsError::NotFound) => Ok(false),
            _ => Err(Error::StorageUnavailable),
        })
    }
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, Error> {
        self.with_vfs(|v| {
            let inode = match v.root_fs.lookup(path) {
                Ok(inode) => inode,
                Err(FsError::NotFound) => return Ok(None),
                _ => return Err(Error::StorageUnavailable),
            };
            if inode.kind != InodeKind::File || inode.size > LIMIT {
                return Err(Error::StorageUnavailable);
            }
            let mut bytes = alloc::vec![0;inode.size];
            let read = v
                .root_fs
                .read(inode.ino, 0, &mut bytes)
                .map_err(|_| Error::StorageUnavailable)?;
            if read != bytes.len() {
                bytes.fill(0);
                return Err(Error::StorageUnavailable);
            }
            Ok(Some(bytes))
        })
    }
    fn remove(&mut self, path: &str) -> Result<(), Error> {
        self.mutation()?;
        self.with_vfs(|v| match v.root_fs.unlink(path) {
            Ok(()) | Err(FsError::NotFound) => Ok(()),
            _ => Err(Error::StorageUnavailable),
        })?;
        if self.exists(path)? {
            return Err(Error::StorageUnavailable);
        }
        Ok(())
    }
    fn write(&mut self, path: &str, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > LIMIT {
            return Err(Error::StorageUnavailable);
        }
        if self.exists(path)? {
            self.remove(path)?;
        }
        self.mutation()?;
        let inode = self.with_vfs(|v| {
            v.root_fs
                .create(path, InodeKind::File)
                .map_err(|_| Error::StorageUnavailable)
        })?;
        self.mutation()?;
        self.with_vfs(|v| {
            v.root_fs
                .write(inode.ino, 0, bytes)
                .map_err(|_| Error::StorageUnavailable)
                .and_then(|n| {
                    if n == bytes.len() {
                        Ok(())
                    } else {
                        Err(Error::StorageUnavailable)
                    }
                })
        })?;
        let mut readback = self.read(path)?.ok_or(Error::StorageUnavailable)?;
        let verified = readback == bytes;
        readback.fill(0);
        if !verified {
            return Err(Error::StorageUnavailable);
        }
        Ok(())
    }
    fn tombstone(&mut self, key: &str) -> Result<(), Error> {
        let path = format!("{ROOT}/records/{key}.revoked");
        // ANY existing marker blocks authority, including an interrupted empty
        // marker. Never replace/delete it before the fresh record verifies.
        if !self.exists(&path)? {
            self.write(&path, b"revoked\n")?;
        }
        if !self.exists(&path)? {
            return Err(Error::StorageUnavailable);
        }
        Ok(())
    }
    fn profile_tombstone(&mut self, app: &str, endpoint: &str) -> Result<(), Error> {
        let marker = format!("{ROOT}/profiles/{}.revoked", key(app, endpoint)?);
        if !self.exists(&marker)? { self.write(&marker, b"revoked\n")?; }
        if !self.exists(&marker)? { return Err(Error::StorageUnavailable); }
        Ok(())
    }
    pub fn forget_profile(&mut self, app: &str, endpoint: &str) -> Result<(), Error> {
        let key = key(app, endpoint)?;
        self.ensure_dirs()?;
        self.profile_tombstone(app, endpoint)?;
        self.remove(&format!("{ROOT}/profiles/{key}.json"))
    }
    /// Private provisioning port. This is not exposed to model tools or apps.
    pub fn save_profile(&mut self, app: &str, profile: &ClientProfile) -> Result<SecretStorage, Error> {
        valid_endpoint(&profile.endpoint)?;
        valid_endpoint(&profile.issuer)?;
        if !valid_profile(profile) { return Err(Error::InvalidRequest); }
        self.ensure_dirs()?;
        self.profile_tombstone(app, &profile.endpoint)?;
        let path = format!("{ROOT}/profiles/{}.json", key(app, &profile.endpoint)?);
        let mut bytes = serde_json::to_vec(profile).map_err(|_| Error::StorageUnavailable)?;
        let result = self.write(&path, &bytes);
        bytes.fill(0);
        result?;
        self.remove(&format!("{ROOT}/profiles/{}.revoked", key(app, &profile.endpoint)?))?;
        self.storage()
    }
    fn storage(&self) -> Result<SecretStorage, Error> {
        self.with_vfs(|v| {
            Ok(if v.backend_name == "CobaltFS" {
                SecretStorage::OsPrivateStore
            } else {
                SecretStorage::Volatile
            })
        })
    }
}
fn hash(bytes: &[u8]) -> String {
    liballoy::net::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn key(app: &str, endpoint: &str) -> Result<String, Error> {
    if app.is_empty()
        || app.len() > 128
        || !app
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidRequest);
    }
    valid_endpoint(endpoint)?;
    Ok(hash(format!("{app}\0{endpoint}").as_bytes()))
}
fn valid_endpoint(endpoint: &str) -> Result<(), Error> {
    if endpoint.len() > 8192 || endpoint.contains('#')
        || alloyos_mcp::transport::remote_mcp_authority(endpoint).is_none() {
        return Err(Error::InvalidRequest);
    }
    Ok(())
}
fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= alloyos_mcp::transport::MAX_BEARER_BYTES
        && !value.bytes().any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
}
fn valid_profile(profile: &ClientProfile) -> bool {
    profile.validate().is_ok()
}
fn valid_account(account: &PrivateAccount) -> bool {
    valid_secret(&account.access_token)
        && alloyos_mcp::account::valid_oauth_resource_binding(&account.endpoint,account.oauth_resource.as_deref().unwrap_or(&account.endpoint))
        && account.refresh_token.as_ref().is_none_or(|s| valid_secret(s))
        && valid_endpoint(&account.issuer).is_ok() && !account.issuer.contains('?')
        && valid_endpoint(&account.token_endpoint).is_ok()
        && account.revocation_endpoint.as_ref().is_none_or(|s| valid_endpoint(s).is_ok())
        && !account.client_id.is_empty() && account.client_id.len() <= 512
        && alloyos_mcp::account::valid_scopes(&account.scopes)
}
impl CredentialStore for KernelCredentialStore {
    fn load(&mut self, app: &str, endpoint: &str) -> Result<Option<PrivateAccount>, Error> {
        let key = key(app, endpoint)?;
        if self.exists(&format!("{ROOT}/records/{key}.revoked"))? {
            return Ok(None);
        }
        let Some(mut bytes) = self.read(&format!("{ROOT}/records/{key}.json"))? else {
            return Ok(None);
        };
        let decoded = serde_json::from_slice::<PrivateAccount>(&bytes);
        bytes.fill(0);
        let account = decoded.map_err(|_| Error::StorageUnavailable)?;
        if account.app_id != app
            || account.endpoint != endpoint

            || !valid_account(&account)
        {
            return Err(Error::StorageUnavailable);
        }
        alloyos_mcp::account::CredentialReference::new(account.reference.clone())?;
        Ok(Some(account))
    }
    fn save(&mut self, account: &PrivateAccount) -> Result<SecretStorage, Error> {
        let key = key(&account.app_id, &account.endpoint)?;
        alloyos_mcp::account::CredentialReference::new(account.reference.clone())?;
        if !valid_account(account) { return Err(Error::InvalidRequest); }
        self.ensure_dirs()?;
        self.tombstone(&key)?;
        let path = format!("{ROOT}/records/{key}.json");
        let mut bytes = serde_json::to_vec(account).map_err(|_| Error::StorageUnavailable)?;
        let result = self.write(&path, &bytes);
        bytes.fill(0);
        result?;
        self.remove(&format!("{ROOT}/records/{key}.revoked"))?;
        self.storage()
    }
    fn forget(&mut self, app: &str, endpoint: &str) -> Result<(), Error> {
        let key = key(app, endpoint)?;
        self.ensure_dirs()?;
        self.tombstone(&key)?;
        self.remove(&format!("{ROOT}/records/{key}.json"))
    }
    fn save_profile(&mut self, app: &str, profile: &ClientProfile) -> Result<SecretStorage, Error> {
        KernelCredentialStore::save_profile(self, app, profile)
    }
    fn forget_profile(&mut self, app: &str, endpoint: &str) -> Result<(), Error> {
        KernelCredentialStore::forget_profile(self, app, endpoint)
    }
    fn profile(&mut self, app: &str, endpoint: &str) -> Result<Option<ClientProfile>, Error> {
        let key = key(app, endpoint)?;
        if self.exists(&format!("{ROOT}/profiles/{key}.revoked"))? { return Ok(None); }
        let path = format!("{ROOT}/profiles/{key}.json");
        let Some(mut bytes) = self.read(&path)? else {
            return Ok(None);
        };
        let decoded = serde_json::from_slice::<ClientProfile>(&bytes);
        bytes.fill(0);
        let profile = decoded.map_err(|_| Error::StorageUnavailable)?;
        if profile.endpoint != endpoint || !valid_profile(&profile) {
            return Err(Error::StorageUnavailable);
        }

        Ok(Some(profile))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::{cobaltfs::CobaltFS, tests::tests::MemBlockDevice};
    const ENDPOINT: &str = "https://service.example/mcp";
    fn account() -> PrivateAccount {
        PrivateAccount {
            app_id: "mail".into(), endpoint: ENDPOINT.into(), oauth_resource: None,
            issuer: "https://issuer.example".into(), client_id: "native-client".into(), issued_at: 1_900_000_000,
            reference: "abcdefghijklmnopqrstuvwxyz0123456789".into(),
            access_token: "synthetic-private-token".into(), refresh_token: Some("synthetic-refresh".into()),
            token_endpoint: "https://issuer.example/token".into(), revocation_endpoint: None,
            scopes: alloc::vec!["read".into()], expires_at: Some(2_000_000_000),
        }
    }
    fn mount(device: Arc<MemBlockDevice>) -> KernelCredentialStore {
        KernelCredentialStore::from_vfs(Arc::new(Mutex::new(Vfs::with_root(
            Arc::new(CobaltFS::mount(device).unwrap()), "CobaltFS"))))
    }
    #[test]
    fn cobalt_remount_preserves_binding_and_durable_disconnect() {
        let disk = MemBlockDevice::new_formatted_large();
        let mut store = mount(disk.clone());
        assert_eq!(store.save(&account()).unwrap(), SecretStorage::OsPrivateStore);
        drop(store);
        let mut store = mount(disk.clone());
        let loaded = store.load("mail", ENDPOINT).unwrap().unwrap();
        assert_eq!(loaded.reference, account().reference);
        assert_eq!(loaded.expires_at, Some(2_000_000_000));
        assert!(store.load("other", ENDPOINT).unwrap().is_none());
        assert!(store.load("mail", "https://service.example/other").unwrap().is_none());
        store.forget("mail", ENDPOINT).unwrap();
        drop(store);
        assert!(mount(disk).load("mail", ENDPOINT).unwrap().is_none());
    }
    fn client_profile() -> ClientProfile {
        ClientProfile {
            endpoint: ENDPOINT.into(), issuer: "https://issuer.example".into(),
            client_id: "native-client".into(), allowed_scopes: alloc::vec!["read".into()],
            loopback_port: 8381, client_secret: Some("synthetic-private-client-secret".into()),
            authorization_options: Default::default(),
        }
    }
    #[test]
    fn client_profiles_are_private_app_and_resource_bound_after_remount() {
        let disk = MemBlockDevice::new_formatted_large();
        let mut profile = client_profile();
        profile.issuer="https://accounts.google.com".into();
        profile.authorization_options=alloyos_mcp::account::AuthorizationOptions::GoogleOffline{reprompt_consent:true};
        let mut store = mount(disk.clone());
        assert_eq!(store.save_profile("mail", &profile).unwrap(), SecretStorage::OsPrivateStore);
        drop(store);
        let mut store = mount(disk.clone());
        assert_eq!(store.profile("mail", ENDPOINT).unwrap().unwrap().client_secret, profile.client_secret);
        assert_eq!(store.profile("mail", ENDPOINT).unwrap().unwrap().authorization_options,profile.authorization_options);
        assert!(store.profile("other", ENDPOINT).unwrap().is_none());
        assert!(store.profile("mail", "https://service.example/other").unwrap().is_none());
        {
            let vfs = store.instance.as_ref().unwrap().lock();
            assert!(!vfs.readdir("/system").unwrap().iter().any(|s| s.starts_with("accounts")));
            assert!(matches!(vfs.readdir("/system/accounts/profiles"), Err(FsError::PermissionDenied)));
        }
        assert!(!format!("{profile:?}").contains("synthetic-private-client-secret"));
        store.forget_profile("mail", ENDPOINT).unwrap();
        drop(store);
        assert!(mount(disk).profile("mail", ENDPOINT).unwrap().is_none());
    }
    #[test]
    fn interrupted_profile_removal_or_replacement_does_not_restore_old_secret() {
        for fault in 1..=3 {
            let disk = MemBlockDevice::new_formatted_large();
            let mut store = mount(disk.clone());
            store.save_profile("mail", &client_profile()).unwrap();
            store.mutations = 0; store.fail_at = Some(fault);
            assert_eq!(store.forget_profile("mail", ENDPOINT), Err(Error::StorageUnavailable));
            drop(store);
            assert_eq!(mount(disk).profile("mail", ENDPOINT).unwrap().is_none(), fault > 1);
        }
        for fault in 1..=3 {
            let disk = MemBlockDevice::new_formatted_large();
            let mut store = mount(disk.clone());
            store.save_profile("mail", &client_profile()).unwrap();
            store.forget_profile("mail", ENDPOINT).unwrap();
            store.mutations = 0; store.fail_at = Some(fault);
            assert_eq!(store.save_profile("mail", &client_profile()), Err(Error::StorageUnavailable));
            drop(store);
            assert!(mount(disk).profile("mail", ENDPOINT).unwrap().is_none());
        }
    }
    #[test]
    fn interrupted_disconnect_never_revives_after_marker_creation() {
        // Each boundary is reconstructed from the actual on-disk filesystem.
        for fault in 1..=3 {
            let disk = MemBlockDevice::new_formatted_large();
            let mut store = mount(disk.clone());
            store.save(&account()).unwrap();
            store.mutations = 0;
            store.fail_at = Some(fault);
            assert_eq!(store.forget("mail", ENDPOINT), Err(Error::StorageUnavailable));
            drop(store);
            let loaded = mount(disk).load("mail", ENDPOINT).unwrap();
            // Before marker creation no deletion is acknowledged; after it,
            // even an interrupted empty marker denies the previous authority.
            assert_eq!(loaded.is_none(), fault > 1);
        }
    }
    #[test]
    fn interrupted_reauthorization_keeps_disconnected_authority_blocked() {
        for fault in 1..=4 {
            let disk = MemBlockDevice::new_formatted_large();
            let mut store = mount(disk.clone());
            store.save(&account()).unwrap();
            store.forget("mail", ENDPOINT).unwrap();
            store.mutations = 0;
            store.fail_at = Some(fault);
            let result = store.save(&account());
            drop(store);
            let loaded = mount(disk).load("mail", ENDPOINT).unwrap();
            if result.is_err() { assert!(loaded.is_none()); }
        }
    }
    #[test]
    fn public_vfs_hides_private_records_and_normalized_aliases() {
        let mut store = KernelCredentialStore::from_vfs(Arc::new(Mutex::new(Vfs::new())));
        assert_eq!(store.save(&account()).unwrap(), SecretStorage::Volatile);
        let vfs = store.instance.as_ref().unwrap().lock();
        assert!(!vfs.readdir("/system").unwrap().iter().any(|s| s.starts_with("accounts")));
        for path in ["/system/accounts", "/system/../system/accounts/records", "/system/accounts/records/token"] {
            assert!(matches!(vfs.lookup(path), Err(FsError::PermissionDenied)));
            assert!(matches!(vfs.create(path, InodeKind::File), Err(FsError::PermissionDenied)));
            assert!(matches!(vfs.unlink(path), Err(FsError::PermissionDenied)));
        }
        assert!(!format!("{store:?}").contains("synthetic-private-token"));
    }
    #[test]
    fn corrupt_current_record_fails_closed_without_older_fallback() {
        let disk = MemBlockDevice::new_formatted_large();
        let mut store = mount(disk.clone());
        store.save(&account()).unwrap();
        store.write(&format!("{ROOT}/records/{}.json", key("mail", ENDPOINT).unwrap()), b"corrupt").unwrap();
        drop(store);
        assert!(matches!(mount(disk).load("mail", ENDPOINT), Err(Error::StorageUnavailable)));
    }
    #[test]
    fn validated_origin_audience_survives_remount_without_widening_transport_binding() {
        let disk=MemBlockDevice::new_formatted_large();let mut record=account();record.oauth_resource=Some("https://service.example".into());
        mount(disk.clone()).save(&record).unwrap();let mut restored=mount(disk.clone());
        assert_eq!(restored.load("mail",ENDPOINT).unwrap().unwrap().oauth_resource,record.oauth_resource);
        for (app,endpoint) in [("other",ENDPOINT),("mail","https://service.example/other"),("mail","https://service.example/mcp?tenant=other")] {assert!(restored.load(app,endpoint).unwrap().is_none());}
        for resource in ["https://service.example/parent","https://other.example","https://service.example?tenant=other"] {
            record.oauth_resource=Some(resource.into());assert_eq!(restored.save(&record),Err(Error::InvalidRequest));
        }
        let path=format!("{ROOT}/records/{}.json",key("mail",ENDPOINT).unwrap());
        restored.write(&path,&serde_json::to_vec(&record).unwrap()).unwrap();drop(restored);
        assert!(matches!(mount(disk).load("mail",ENDPOINT),Err(Error::StorageUnavailable)));
    }

}
