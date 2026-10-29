//! 网易云 `weapi` 参数加密。
//!
//! 算法与官方网页端一致，分三步：
//! 1. 用固定密钥 `0CoJUm6Qyw8W8jud` 对 JSON 做一次 AES-128-CBC（PKCS#7）；
//! 2. 再随机生成 16 位密钥，对第一步的 Base64 结果做第二次 AES；
//! 3. 用网易的公钥对“反转后的随机密钥”做无填充 RSA，得到 `encSecKey`。
//!
//! 随机密钥只存在一次请求里，服务端解密后即可还原参数。

use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use num_bigint::BigUint;
use rand::Rng;
use serde::Serialize;

use crate::ApiError;

type Aes128CbcEnc = cbc::Encryptor<aes::Aes128>;

pub const IV: &str = "0102030405060708";
pub const PRESET_KEY: &str = "0CoJUm6Qyw8W8jud";

/// 网易云 Web 端 RSA 公钥的模数（无填充，指数固定 65537）。
const MODULUS_HEX: &str = concat!(
    "00e0b509f6259df8642dbc35662901477df22677ec152b5ff68ace615bb7b725152",
    "b3ab17a876aea8a5aa76d2e417629ec4ee341f56135fccf695280104e0312ecbda9",
    "2557c93870114af6c9d05c4f7f0c3685b7a46bee255932575cce10b424d813cfe4",
    "875d3e82047b97ddef52741d546b8e289dc6935b3ece0462db0a22b8e7"
);

const BASE62: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

#[derive(Debug, Clone)]
pub struct WeapiParams {
    pub params: String,
    pub enc_sec_key: String,
}

/// 把任意可序列化的参数加密成 `params` / `encSecKey` 表单字段。
pub fn weapi<T: Serialize>(payload: &T) -> Result<WeapiParams, ApiError> {
    let json = serde_json::to_string(payload)
        .map_err(|error| ApiError::Crypto(format!("序列化参数失败：{error}")))?;

    let secret = random_secret();
    let first = aes_cbc_base64(json.as_bytes(), PRESET_KEY.as_bytes())?;
    let params = aes_cbc_base64(first.as_bytes(), secret.as_bytes())?;
    let enc_sec_key = rsa_encrypt(&secret)?;

    Ok(WeapiParams {
        params,
        enc_sec_key,
    })
}

/// 便捷方法：直接对一组键值对加密。
fn random_secret() -> String {
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| BASE62[rng.gen_range(0..BASE62.len())] as char)
        .collect()
}

fn aes_cbc_base64(plain: &[u8], key: &[u8]) -> Result<String, ApiError> {
    let mut buffer = vec![0u8; plain.len() + 16];
    buffer[..plain.len()].copy_from_slice(plain);

    let encrypted = Aes128CbcEnc::new_from_slices(key, IV.as_bytes())
        .map_err(|error| ApiError::Crypto(format!("AES 密钥无效：{error}")))?
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, plain.len())
        .map_err(|_| ApiError::Crypto("AES 填充失败".to_string()))?;

    Ok(BASE64.encode(encrypted))
}

fn rsa_encrypt(secret: &str) -> Result<String, ApiError> {
    let reversed: String = secret.chars().rev().collect();
    let message = BigUint::from_bytes_be(reversed.as_bytes());
    let exponent = BigUint::from(65537u32);
    let modulus = BigUint::parse_bytes(MODULUS_HEX.as_bytes(), 16)
        .ok_or_else(|| ApiError::Crypto("RSA 模数无效".to_string()))?;

    let encrypted = message.modpow(&exponent, &modulus);
    Ok(format!("{:0>256}", encrypted.to_str_radix(16)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::{BlockDecryptMut, KeyIvInit};
    use serde_json::json;

    type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

    fn decrypt(ciphertext_base64: &str, key: &[u8]) -> Vec<u8> {
        let ciphertext = BASE64.decode(ciphertext_base64).expect("base64");
        Aes128CbcDec::new_from_slices(key, IV.as_bytes())
            .expect("key")
            .decrypt_padded_vec_mut::<Pkcs7>(&ciphertext)
            .expect("padding")
    }

    #[test]
    fn aes_matches_cbc_pkcs7() {
        let encrypted =
            aes_cbc_base64("hello 网易云".as_bytes(), PRESET_KEY.as_bytes()).expect("encrypt");
        assert_eq!(
            decrypt(&encrypted, PRESET_KEY.as_bytes()),
            "hello 网易云".as_bytes()
        );
    }

    #[test]
    fn secret_is_16_chars() {
        for _ in 0..32 {
            let secret = random_secret();
            assert_eq!(secret.len(), 16);
            assert!(secret.bytes().all(|byte| BASE62.contains(&byte)));
        }
    }

    #[test]
    fn rsa_output_is_padded_hex() {
        let encrypted = rsa_encrypt("0123456789abcdef").expect("rsa");
        assert_eq!(encrypted.len(), 256);
        assert!(encrypted
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[test]
    fn weapi_has_expected_shape() {
        let params = weapi(&json!({ "type": 1 })).expect("weapi");
        assert_eq!(params.enc_sec_key.len(), 256);
        let raw = BASE64.decode(&params.params).expect("base64");
        assert_eq!(raw.len() % 16, 0);
    }
}
