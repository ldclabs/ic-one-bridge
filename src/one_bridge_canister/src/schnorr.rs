use ic_cdk_management_canister as mgt;
use serde_bytes::ByteBuf;

use crate::{helper::format_error, types::PublicKeyOutput};

/// Every Schnorr key the canister uses is Ed25519: the Solana addresses.
const ALGORITHM: mgt::SchnorrAlgorithm = mgt::SchnorrAlgorithm::Ed25519;

/// Derives the Ed25519 public key at `derivation_path` under `public_key`, the
/// same way the management canister derives the key it signs with.
pub fn derive_schnorr_public_key(
    public_key: &PublicKeyOutput,
    derivation_path: Vec<Vec<u8>>,
) -> Result<PublicKeyOutput, String> {
    let path = ic_ed25519::DerivationPath::new(
        derivation_path
            .into_iter()
            .map(ic_ed25519::DerivationIndex)
            .collect(),
    );

    let chain_code: [u8; 32] = public_key
        .chain_code
        .to_vec()
        .try_into()
        .map_err(format_error)?;
    let pk =
        ic_ed25519::PublicKey::deserialize_raw(&public_key.public_key).map_err(format_error)?;
    let (derived_public_key, derived_chain_code) =
        pk.derive_subkey_with_chain_code(&path, &chain_code);

    Ok(PublicKeyOutput {
        public_key: ByteBuf::from(derived_public_key.serialize_raw()),
        chain_code: ByteBuf::from(derived_chain_code),
    })
}

pub async fn sign_with_schnorr(
    key_name: String,
    derivation_path: Vec<Vec<u8>>,
    message: Vec<u8>,
) -> Result<Vec<u8>, String> {
    let args = mgt::SignWithSchnorrArgs {
        message,
        derivation_path,
        key_id: mgt::SchnorrKeyId {
            algorithm: ALGORITHM,
            name: key_name,
        },
        aux: None,
    };

    let rt = mgt::sign_with_schnorr(&args)
        .await
        .map_err(|err| format!("sign_with_schnorr failed: {:?}", err))?;

    Ok(rt.signature)
}

pub async fn schnorr_public_key(
    key_name: String,
    derivation_path: Vec<Vec<u8>>,
) -> Result<PublicKeyOutput, String> {
    let args = mgt::SchnorrPublicKeyArgs {
        canister_id: None,
        derivation_path,
        key_id: mgt::SchnorrKeyId {
            algorithm: ALGORITHM,
            name: key_name,
        },
    };

    let rt = mgt::schnorr_public_key(&args)
        .await
        .map_err(|err| format!("schnorr_public_key failed {:?}", err))?;
    Ok(PublicKeyOutput {
        public_key: ByteBuf::from(rt.public_key),
        chain_code: ByteBuf::from(rt.chain_code),
    })
}
