//! XXX

use failure;
use std::ffi::CStr;
use std::mem::forget;
use std::ptr;
use std::slice;
use std::io;
use std::io::{Read, Write};
use libc::{uint8_t, c_char, c_int, size_t, ssize_t, c_void, time_t};
use failure::ResultExt;

extern crate sequoia_openpgp as openpgp;
extern crate time;

use self::openpgp::{
    Fingerprint,
    KeyID,
    RevocationStatus,
    TPK,
    TSK,
    Packet,
    packet::{
        Signature,
        Tag,
        PKESK,
        SKESK,
        key::SecretKey,
    },
    crypto::Password,
};
use self::openpgp::packet;
use self::openpgp::parse::{
    Parse,
    PacketParserResult,
    PacketParser,
    PacketParserEOF,
};
use self::openpgp::parse::stream::{
    DecryptionHelper,
    Decryptor,
    Secret,
    VerificationHelper,
    VerificationResult,
    Verifier,
    DetachedVerifier,
};
use self::openpgp::serialize::Serialize;
use self::openpgp::constants::{
    DataFormat,
};

use super::error::Status;
use super::core::Context;

pub mod armor;
pub mod crypto;
pub mod fingerprint;
pub mod keyid;
pub mod packet_pile;
pub mod tpk;

/* openpgp::packet::Tag.  */

/// Returns a human-readable tag name.
///
/// ```c
/// #include <assert.h>
/// #include <string.h>
/// #include <sequoia.h>
///
/// assert (strcmp (sq_tag_to_string (2), "SIGNATURE") == 0);
/// ```
#[no_mangle]
pub extern "system" fn sq_tag_to_string(tag: u8) -> *const c_char {
    match Tag::from(tag) {
        Tag::PKESK => "PKESK\x00",
        Tag::Signature => "SIGNATURE\x00",
        Tag::SKESK => "SKESK\x00",
        Tag::OnePassSig => "ONE PASS SIG\x00",
        Tag::SecretKey => "SECRET KEY\x00",
        Tag::PublicKey => "PUBLIC KEY\x00",
        Tag::SecretSubkey => "SECRET SUBKEY\x00",
        Tag::CompressedData => "COMPRESSED DATA\x00",
        Tag::SED => "SED\x00",
        Tag::Marker => "MARKER\x00",
        Tag::Literal => "LITERAL\x00",
        Tag::Trust => "TRUST\x00",
        Tag::UserID => "USER ID\x00",
        Tag::PublicSubkey => "PUBLIC SUBKEY\x00",
        Tag::UserAttribute => "USER ATTRIBUTE\x00",
        Tag::SEIP => "SEIP\x00",
        Tag::MDC => "MDC\x00",
        _ => "OTHER\x00",
    }.as_bytes().as_ptr() as *const c_char
}

fn revocation_status_to_int(rs: &RevocationStatus) -> c_int {
    match rs {
        RevocationStatus::Revoked(_) => 0,
        RevocationStatus::CouldBe(_) => 1,
        RevocationStatus::NotAsFarAsWeKnow => 2,
    }
}

/// Returns the TPK's revocation status variant.
#[no_mangle]
pub extern "system" fn sq_revocation_status_variant(
    rs: *mut RevocationStatus)
    -> c_int
{
    let rs = ffi_param_move!(rs);
    let variant = revocation_status_to_int(rs.as_ref());
    Box::into_raw(rs);
    variant
}

/// Frees a sq_revocation_status_t.
#[no_mangle]
pub extern "system" fn sq_revocation_status_free(
    rs: Option<&mut RevocationStatus>)
{
    ffi_free!(rs)
}

/* TSK */

/// Generates a new RSA 3072 bit key with UID `primary_uid`.
#[no_mangle]
pub extern "system" fn sq_tsk_new(ctx: *mut Context,
                                  primary_uid: *const c_char,
                                  tsk_out: *mut *mut TSK,
                                  revocation_out: *mut *mut Signature)
    -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    assert!(!primary_uid.is_null());
    let tsk_out = ffi_param_ref_mut!(tsk_out);
    let revocation_out = ffi_param_ref_mut!(revocation_out);
    let primary_uid = unsafe {
        CStr::from_ptr(primary_uid)
    };
    match TSK::new(primary_uid.to_string_lossy()) {
        Ok((tsk, revocation)) => {
            *tsk_out = box_raw!(tsk);
            *revocation_out = box_raw!(revocation);
            Status::Success
        },
        Err(e) => fry_status!(ctx, Err::<(), failure::Error>(e)),
    }
}

/// Frees the TSK.
#[no_mangle]
pub extern "system" fn sq_tsk_free(tsk: Option<&mut TSK>) {
    ffi_free!(tsk)
}

/// Returns a reference to the corresponding TPK.
#[no_mangle]
pub extern "system" fn sq_tsk_tpk(tsk: *const TSK)
                                  -> *const TPK {
    let tsk = ffi_param_ref!(tsk);
    tsk.tpk()
}

/// Converts the TSK into a TPK.
#[no_mangle]
pub extern "system" fn sq_tsk_into_tpk(tsk: *mut TSK)
                                       -> *mut TPK {
    let tsk = ffi_param_move!(tsk);
    box_raw!(tsk.into_tpk())
}


/// Serializes the TSK.
#[no_mangle]
pub extern "system" fn sq_tsk_serialize(ctx: *mut Context,
                                        tsk: *const TSK,
                                        writer: *mut Box<Write>)
                                        -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let tsk = ffi_param_ref!(tsk);
    let writer = ffi_param_ref_mut!(writer);
    fry_status!(ctx, tsk.serialize(writer))
}

/* openpgp::Packet.  */

/// Frees the Packet.
#[no_mangle]
pub extern "system" fn sq_packet_free(p: Option<&mut Packet>) {
    ffi_free!(p)
}

/// Returns the `Packet's` corresponding OpenPGP tag.
///
/// Tags are explained in [Section 4.3 of RFC 4880].
///
///   [Section 4.3 of RFC 4880]: https://tools.ietf.org/html/rfc4880#section-4.3
#[no_mangle]
pub extern "system" fn sq_packet_tag(p: *const Packet)
                                     -> uint8_t {
    let p = ffi_param_ref!(p);
    let tag: u8 = p.tag().into();
    tag as uint8_t
}

/// Returns the parsed `Packet's` corresponding OpenPGP tag.
///
/// Returns the packets tag, but only if it was successfully
/// parsed into the corresponding packet type.  If e.g. a
/// Signature Packet uses some unsupported methods, it is parsed
/// into an `Packet::Unknown`.  `tag()` returns `SQ_TAG_SIGNATURE`,
/// whereas `kind()` returns `0`.
#[no_mangle]
pub extern "system" fn sq_packet_kind(p: *const Packet)
                                      -> uint8_t {
    let p = ffi_param_ref!(p);
    if let Some(kind) = p.kind() {
        kind.into()
    } else {
        0
    }
}

/// Frees the Signature.
#[no_mangle]
pub extern "system" fn sq_signature_free(s: Option<&mut Signature>) {
    ffi_free!(s)
}

/// Converts the signature to a packet.
#[no_mangle]
pub extern "system" fn sq_signature_to_packet(s: *mut Signature)
                                              -> *mut Packet
{
    let s = ffi_param_move!(s);
    box_raw!(s.to_packet())
}

/// Returns the value of the `Signature` packet's Issuer subpacket.
///
/// If there is no Issuer subpacket, this returns NULL.  Note: if
/// there is no Issuer subpacket, but there is an IssuerFingerprint
/// subpacket, this still returns NULL.
#[no_mangle]
pub extern "system" fn sq_signature_issuer(sig: *const packet::Signature)
                                           -> *mut KeyID {
    let sig = ffi_param_ref!(sig);
    maybe_box_raw!(sig.issuer())
}

/// Returns the value of the `Signature` packet's IssuerFingerprint subpacket.
///
/// If there is no IssuerFingerprint subpacket, this returns NULL.
/// Note: if there is no IssuerFingerprint subpacket, but there is an
/// Issuer subpacket, this still returns NULL.
#[no_mangle]
pub extern "system" fn sq_signature_issuer_fingerprint(
    sig: *const packet::Signature)
    -> *mut Fingerprint
{
    let sig = ffi_param_ref!(sig);
    maybe_box_raw!(sig.issuer_fingerprint())
}


/// Returns whether the KeyFlags indicates that the key can be used to
/// make certifications.
#[no_mangle]
pub extern "system" fn sq_signature_can_certify(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().can_certify()
}

/// Returns whether the KeyFlags indicates that the key can be used to
/// make signatures.
#[no_mangle]
pub extern "system" fn sq_signature_can_sign(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().can_sign()
}

/// Returns whether the KeyFlags indicates that the key can be used to
/// encrypt data for transport.
#[no_mangle]
pub extern "system" fn sq_signature_can_encrypt_for_transport(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().can_encrypt_for_transport()
}

/// Returns whether the KeyFlags indicates that the key can be used to
/// encrypt data at rest.
#[no_mangle]
pub extern "system" fn sq_signature_can_encrypt_at_rest(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().can_encrypt_at_rest()
}

/// Returns whether the KeyFlags indicates that the key can be used
/// for authentication.
#[no_mangle]
pub extern "system" fn sq_signature_can_authenticate(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().can_authenticate()
}

/// Returns whether the KeyFlags indicates that the key is a split
/// key.
#[no_mangle]
pub extern "system" fn sq_signature_is_split_key(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().is_split_key()
}

/// Returns whether the KeyFlags indicates that the key is a group
/// key.
#[no_mangle]
pub extern "system" fn sq_signature_is_group_key(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.key_flags().is_group_key()
}


/// Returns whether the signature is alive.
///
/// A signature is alive if the creation date is in the past, and the
/// signature has not expired.
#[no_mangle]
pub extern "system" fn sq_signature_alive(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.signature_alive()
}

/// Returns whether the signature is alive at the specified time.
///
/// A signature is alive if the creation date is in the past, and the
/// signature has not expired at the specified time.
#[no_mangle]
pub extern "system" fn sq_signature_alive_at(sig: *const packet::Signature,
                                             when: time_t)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.signature_alive_at(time::at(time::Timespec::new(when as i64, 0)))
}

/// Returns whether the signature is expired.
#[no_mangle]
pub extern "system" fn sq_signature_expired(sig: *const packet::Signature)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.signature_expired()
}

/// Returns whether the signature is expired at the specified time.
#[no_mangle]
pub extern "system" fn sq_signature_expired_at(sig: *const packet::Signature,
                                               when: time_t)
    -> bool
{
    let sig = ffi_param_ref!(sig);
    sig.signature_expired_at(time::at(time::Timespec::new(when as i64, 0)))
}


/// Clones the key.
#[no_mangle]
pub extern "system" fn sq_p_key_clone(key: *const packet::Key)
                                      -> *mut packet::Key {
    let key = ffi_param_ref!(key);
    box_raw!(key.clone())
}

/// Computes and returns the key's fingerprint as per Section 12.2
/// of RFC 4880.
#[no_mangle]
pub extern "system" fn sq_p_key_fingerprint(key: *const packet::Key)
                                            -> *mut Fingerprint {
    let key = ffi_param_ref!(key);
    box_raw!(key.fingerprint())
}

/// Computes and returns the key's key ID as per Section 12.2 of RFC
/// 4880.
#[no_mangle]
pub extern "system" fn sq_p_key_keyid(key: *const packet::Key)
                                      -> *mut KeyID {
    let key = ffi_param_ref!(key);
    box_raw!(key.keyid())
}

/// Returns whether the key is expired according to the provided
/// self-signature.
///
/// Note: this is with respect to the provided signature, which is not
/// checked for validity.  That is, we do not check whether the
/// signature is a valid self-signature for the given key.
#[no_mangle]
pub extern "system" fn sq_p_key_expired(key: *const packet::Key,
                                      sig: *const packet::Signature)
    -> bool
{
    let key = ffi_param_ref!(key);
    let sig = ffi_param_ref!(sig);

    sig.key_expired(key)
}

/// Like sq_p_key_expired, but at a specific time.
#[no_mangle]
pub extern "system" fn sq_p_key_expired_at(key: *const packet::Key,
                                         sig: *const packet::Signature,
                                         when: time_t)
    -> bool
{
    let key = ffi_param_ref!(key);
    let sig = ffi_param_ref!(sig);

    sig.key_expired_at(key, time::at(time::Timespec::new(when as i64, 0)))
}

/// Returns whether the key is alive according to the provided
/// self-signature.
///
/// A key is alive if the creation date is in the past, and the key
/// has not expired.
///
/// Note: this is with respect to the provided signature, which is not
/// checked for validity.  That is, we do not check whether the
/// signature is a valid self-signature for the given key.
#[no_mangle]
pub extern "system" fn sq_p_key_alive(key: *const packet::Key,
                                      sig: *const packet::Signature)
    -> bool
{
    let key = ffi_param_ref!(key);
    let sig = ffi_param_ref!(sig);

    sig.key_alive(key)
}

/// Like sq_p_key_alive, but at a specific time.
#[no_mangle]
pub extern "system" fn sq_p_key_alive_at(key: *const packet::Key,
                                         sig: *const packet::Signature,
                                         when: time_t)
    -> bool
{
    let key = ffi_param_ref!(key);
    let sig = ffi_param_ref!(sig);

    sig.key_alive_at(key, time::at(time::Timespec::new(when as i64, 0)))
}

/// Returns the key's creation time.
#[no_mangle]
pub extern "system" fn sq_p_key_creation_time(key: *const packet::Key)
    -> u32
{
    let key = ffi_param_ref!(key);
    let ct = key.creation_time();

    ct.to_timespec().sec as u32
}

/// Returns the key's public key algorithm.
#[no_mangle]
pub extern "system" fn sq_p_key_public_key_algo(key: *const packet::Key)
    -> c_int
{
    let key = ffi_param_ref!(key);
    let pk_algo : u8 = key.pk_algo().into();
    pk_algo as c_int
}

/// Returns the public key's size in bits.
#[no_mangle]
pub extern "system" fn sq_p_key_public_key_bits(key: *const packet::Key)
    -> c_int
{
    use self::openpgp::crypto::mpis::PublicKey::*;

    let key = ffi_param_ref!(key);
    match key.mpis() {
        RSA { e: _, n } => n.bits as c_int,
        DSA { p: _, q: _, g: _, y } => y.bits as c_int,
        Elgamal { p: _, g: _, y } => y.bits as c_int,
        EdDSA { curve: _, q } => q.bits as c_int,
        ECDSA { curve: _, q } =>  q.bits as c_int,
        ECDH { curve: _, q, hash: _, sym: _ } =>  q.bits as c_int,
        Unknown { mpis: _, rest: _ } => 0,
    }
}

/// Returns the value of the User ID Packet.
///
/// The returned pointer is valid until `uid` is deallocated.  If
/// `value_len` is not `NULL`, the size of value is stored there.
#[no_mangle]
pub extern "system" fn sq_user_id_value(uid: *const Packet,
                                        value_len: Option<&mut size_t>)
                                        -> *const uint8_t {
    let uid = ffi_param_ref!(uid);
    if let &Packet::UserID(ref uid) = uid {
        if let Some(p) = value_len {
            *p = uid.userid().len();
        }
        uid.userid().as_ptr()
    } else {
        panic!("Not a UserID packet");
    }
}

/// Returns the value of the User Attribute Packet.
///
/// The returned pointer is valid until `ua` is deallocated.  If
/// `value_len` is not `NULL`, the size of value is stored there.
#[no_mangle]
pub extern "system" fn sq_user_attribute_value(ua: *const Packet,
                                               value_len: Option<&mut size_t>)
                                               -> *const uint8_t {
    let ua = ffi_param_ref!(ua);
    if let &Packet::UserAttribute(ref ua) = ua {
        if let Some(p) = value_len {
            *p = ua.user_attribute().len();
        }
        ua.user_attribute().as_ptr()
    } else {
        panic!("Not a UserAttribute packet");
    }
}

/// Returns the session key.
///
/// `key` of size `key_len` must be a buffer large enough to hold the
/// session key.  If `key` is NULL, or not large enough, then the key
/// is not written to it.  Either way, `key_len` is set to the size of
/// the session key.
#[no_mangle]
pub extern "system" fn sq_skesk_decrypt(ctx: *mut Context,
                                        skesk: *const Packet,
                                        password: *const uint8_t,
                                        password_len: size_t,
                                        algo: *mut uint8_t, // XXX
                                        key: *mut uint8_t,
                                        key_len: *mut size_t)
                                        -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let skesk = ffi_param_ref!(skesk);
    assert!(!password.is_null());
    let password = unsafe {
        slice::from_raw_parts(password, password_len as usize)
    };
    let algo = ffi_param_ref_mut!(algo);
    let key_len = ffi_param_ref_mut!(key_len);

    if let &Packet::SKESK(ref skesk) = skesk {
        match skesk.decrypt(&password.to_owned().into()) {
            Ok((a, k)) => {
                *algo = a.into();
                if !key.is_null() && *key_len >= k.len() {
                    unsafe {
                        ::std::ptr::copy(k.as_ptr(),
                                         key,
                                         k.len());
                    }
                }
                *key_len = k.len();
                Status::Success
            },
            Err(e) => fry_status!(ctx, Err::<(), failure::Error>(e)),
        }
    } else {
        panic!("Not a SKESK packet");
    }
}

/// Returns the PKESK's recipient.
///
/// The return value is a reference ot a `KeyID`.  The caller must not
/// modify or free it.
#[no_mangle]
pub extern "system" fn sq_pkesk_recipient(pkesk: *const PKESK)
                                          -> *const KeyID {
    let pkesk = ffi_param_ref!(pkesk);
    pkesk.recipient()
}

/// Returns the session key.
///
/// `key` of size `key_len` must be a buffer large enough to hold the
/// session key.  If `key` is NULL, or not large enough, then the key
/// is not written to it.  Either way, `key_len` is set to the size of
/// the session key.
#[no_mangle]
pub extern "system" fn sq_pkesk_decrypt(ctx: *mut Context,
                                        pkesk: *const PKESK,
                                        secret_key: *const packet::Key,
                                        algo: *mut uint8_t, // XXX
                                        key: *mut uint8_t,
                                        key_len: *mut size_t)
                                        -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let pkesk = ffi_param_ref!(pkesk);
    let secret_key = ffi_param_ref!(secret_key);
    let algo = ffi_param_ref_mut!(algo);
    let key_len = ffi_param_ref_mut!(key_len);

    if let Some(SecretKey::Unencrypted{ mpis: ref secret_part }) = secret_key.secret() {
        match pkesk.decrypt(secret_key, secret_part) {
            Ok((a, k)) => {
                *algo = a.into();
                if !key.is_null() && *key_len >= k.len() {
                    unsafe {
                        ::std::ptr::copy(k.as_ptr(),
                                         key,
                                         k.len());
                    }
                }
                *key_len = k.len();
                Status::Success
            },
            Err(e) => fry_status!(ctx, Err::<(), failure::Error>(e)),
        }
    } else {
        // XXX: Better message.
        panic!("No secret parts");
    }
}

/* openpgp::parse.  */

/// Starts parsing OpenPGP packets stored in a `sq_reader_t`
/// object.
///
/// This function returns a `PacketParser` for the first packet in
/// the stream.
#[no_mangle]
pub extern "system" fn sq_packet_parser_from_reader<'a>
    (ctx: *mut Context, reader: *mut Box<'a + Read>)
     -> *mut PacketParserResult<'a> {
    let ctx = ffi_param_ref_mut!(ctx);
    let reader = ffi_param_ref_mut!(reader);
    fry_box!(ctx, PacketParser::from_reader(reader))
}

/// Starts parsing OpenPGP packets stored in a file named `path`.
///
/// This function returns a `PacketParser` for the first packet in
/// the stream.
#[no_mangle]
pub extern "system" fn sq_packet_parser_from_file
    (ctx: *mut Context, filename: *const c_char)
     -> *mut PacketParserResult<'static> {
    let ctx = ffi_param_ref_mut!(ctx);
    assert!(! filename.is_null());
    let filename = unsafe {
        CStr::from_ptr(filename).to_string_lossy().into_owned()
    };
    fry_box!(ctx, PacketParser::from_file(&filename))
}

/// Starts parsing OpenPGP packets stored in a buffer.
///
/// This function returns a `PacketParser` for the first packet in
/// the stream.
#[no_mangle]
pub extern "system" fn sq_packet_parser_from_bytes
    (ctx: *mut Context, b: *const uint8_t, len: size_t)
     -> *mut PacketParserResult<'static> {
    let ctx = ffi_param_ref_mut!(ctx);
    assert!(!b.is_null());
    let buf = unsafe {
        slice::from_raw_parts(b, len as usize)
    };

    fry_box!(ctx, PacketParser::from_bytes(buf))
}

/// Frees the packet parser result
#[no_mangle]
pub extern "system" fn sq_packet_parser_result_free(
    ppr: Option<&mut PacketParserResult>)
{
    ffi_free!(ppr)
}

/// Frees the packet parser.
#[no_mangle]
pub extern "system" fn sq_packet_parser_free(pp: Option<&mut PacketParser>) {
    ffi_free!(pp)
}

/// Frees the packet parser EOF object.
#[no_mangle]
pub extern "system" fn sq_packet_parser_eof_is_message(
    eof: *const PacketParserEOF) -> bool
{
    let eof = ffi_param_ref!(eof);

    eof.is_message()
}

/// Frees the packet parser EOF object.
#[no_mangle]
pub extern "system" fn sq_packet_parser_eof_free
    (eof: Option<&mut PacketParserEOF>)
{
    ffi_free!(eof)
}

/// Returns a reference to the packet that is being parsed.
#[no_mangle]
pub extern "system" fn sq_packet_parser_packet
    (pp: *const PacketParser)
     -> *const Packet {
    let pp = ffi_param_ref!(pp);
    &pp.packet
}

/// Returns the current packet's recursion depth.
///
/// A top-level packet has a recursion depth of 0.  Packets in a
/// top-level container have a recursion depth of 1, etc.
#[no_mangle]
pub extern "system" fn sq_packet_parser_recursion_depth
    (pp: *const PacketParser)
     -> uint8_t {
    let pp = ffi_param_ref!(pp);
    pp.recursion_depth() as u8
}

/// Finishes parsing the current packet and starts parsing the
/// following one.
///
/// This function finishes parsing the current packet.  By
/// default, any unread content is dropped.  (See
/// [`PacketParsererBuilder`] for how to configure this.)  It then
/// creates a new packet parser for the following packet.  If the
/// current packet is a container, this function does *not*
/// recurse into the container, but skips any packets it contains.
/// To recurse into the container, use the [`recurse()`] method.
///
///   [`PacketParsererBuilder`]: parse/struct.PacketParserBuilder.html
///   [`recurse()`]: #method.recurse
///
/// The return value is a tuple containing:
///
///   - A `Packet` holding the fully processed old packet;
///
///   - The old packet's recursion depth;
///
///   - A `PacketParser` holding the new packet;
///
///   - And, the recursion depth of the new packet.
///
/// A recursion depth of 0 means that the packet is a top-level
/// packet, a recursion depth of 1 means that the packet is an
/// immediate child of a top-level-packet, etc.
///
/// Since the packets are serialized in depth-first order and all
/// interior nodes are visited, we know that if the recursion
/// depth is the same, then the packets are siblings (they have a
/// common parent) and not, e.g., cousins (they have a common
/// grandparent).  This is because, if we move up the tree, the
/// only way to move back down is to first visit a new container
/// (e.g., an aunt).
///
/// Using the two positions, we can compute the change in depth as
/// new_depth - old_depth.  Thus, if the change in depth is 0, the
/// two packets are siblings.  If the value is 1, the old packet
/// is a container, and the new packet is its first child.  And,
/// if the value is -1, the new packet is contained in the old
/// packet's grandparent.  The idea is illustrated below:
///
/// ```text
///             ancestor
///             |       \
///            ...      -n
///             |
///           grandparent
///           |          \
///         parent       -1
///         |      \
///      packet    0
///         |
///         1
/// ```
///
/// Note: since this function does not automatically recurse into
/// a container, the change in depth will always be non-positive.
/// If the current container is empty, this function DOES pop that
/// container off the container stack, and returns the following
/// packet in the parent container.
///
/// The items of the tuple are returned in out-parameters.  If you do
/// not wish to receive the value, pass `NULL` as the parameter.
///
/// Consumes the given packet parser.
#[no_mangle]
pub extern "system" fn sq_packet_parser_next<'a>
    (ctx: *mut Context,
     pp: *mut PacketParser<'a>,
     old_packet: Option<&mut *mut Packet>,
     ppr: Option<&mut *mut PacketParserResult<'a>>)
     -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let pp = ffi_param_move!(pp);

    match pp.next() {
        Ok((old_p, new_ppr)) => {
            if let Some(p) = old_packet {
                *p = box_raw!(old_p);
            }
            if let Some(p) = ppr {
                *p = box_raw!(new_ppr);
            }
            Status::Success
        },
        Err(e) => fry_status!(ctx, Err::<(), failure::Error>(e)),
    }
}

/// Finishes parsing the current packet and starts parsing the
/// next one, recursing if possible.
///
/// This method is similar to the [`next()`] method (see that
/// method for more details), but if the current packet is a
/// container (and we haven't reached the maximum recursion depth,
/// and the user hasn't started reading the packet's contents), we
/// recurse into the container, and return a `PacketParser` for
/// its first child.  Otherwise, we return the next packet in the
/// packet stream.  If this function recurses, then the new
/// packet's recursion depth will be `last_recursion_depth() + 1`;
/// because we always visit interior nodes, we can't recurse more
/// than one level at a time.
///
///   [`next()`]: #method.next
///
/// The items of the tuple are returned in out-parameters.  If you do
/// not wish to receive the value, pass `NULL` as the parameter.
///
/// Consumes the given packet parser.
#[no_mangle]
pub extern "system" fn sq_packet_parser_recurse<'a>
    (ctx: *mut Context,
     pp: *mut PacketParser<'a>,
     old_packet: Option<&mut *mut Packet>,
     ppr: Option<&mut *mut PacketParserResult<'a>>)
     -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let pp = ffi_param_move!(pp);

    match pp.recurse() {
        Ok((old_p, new_ppr)) => {
            if let Some(p) = old_packet {
                *p = box_raw!(old_p);
            }
            if let Some(p) = ppr {
                *p = box_raw!(new_ppr);
            }
            Status::Success
        },
        Err(e) => fry_status!(ctx, Err::<(), failure::Error>(e)),
    }
}

/// Causes the PacketParser to buffer the packet's contents.
///
/// The packet's contents are stored in `packet.content`.  In
/// general, you should avoid buffering a packet's content and
/// prefer streaming its content unless you are certain that the
/// content is small.
#[no_mangle]
pub extern "system" fn sq_packet_parser_buffer_unread_content<'a>
    (ctx: *mut Context,
     pp: *mut PacketParser<'a>,
     len: *mut usize)
     -> *const uint8_t {
    let ctx = ffi_param_ref_mut!(ctx);
    let pp = ffi_param_ref_mut!(pp);
    let len = ffi_param_ref_mut!(len);
    let buf = fry!(ctx, pp.buffer_unread_content());
    *len = buf.len();
    buf.as_ptr()
}

/// Finishes parsing the current packet.
///
/// By default, this drops any unread content.  Use, for instance,
/// `PacketParserBuild` to customize the default behavior.
#[no_mangle]
pub extern "system" fn sq_packet_parser_finish<'a>
    (ctx: *mut Context, pp: *mut PacketParser<'a>,
     packet: Option<&mut *const Packet>)
     -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    let pp = ffi_param_ref_mut!(pp);
    match pp.finish() {
        Ok(p) => {
            if let Some(out_p) = packet {
                *out_p = p;
            }
            Status::Success
        },
        Err(e) => {
            let status = Status::from(&e);
            ctx.e = Some(e);
            status
        },
    }
}

/// Tries to decrypt the current packet.
///
/// On success, this function pushes one or more readers onto the
/// `PacketParser`'s reader stack, and sets the packet's
/// `decrypted` flag.
///
/// If this function is called on a packet that does not contain
/// encrypted data, or some of the data was already read, then it
/// returns `Error::InvalidOperation`.
#[no_mangle]
pub extern "system" fn sq_packet_parser_decrypt<'a>
    (ctx: *mut Context,
     pp: *mut PacketParser<'a>,
     algo: uint8_t, // XXX
     key: *const uint8_t, key_len: size_t)
     -> Status {
    let ctx = ffi_param_ref_mut!(ctx);
    let pp = ffi_param_ref_mut!(pp);
    let key = unsafe {
        slice::from_raw_parts(key, key_len as usize)
    };
    let key = key.to_owned().into();
    fry_status!(ctx, pp.decrypt((algo as u8).into(), &key))
}


/* PacketParserResult.  */

/// Returns the current packet's tag.
///
/// This is a convenience function to inspect the containing packet,
/// without turning the `PacketParserResult` into a `PacketParser`.
///
/// This function does not consume the ppr.
///
/// Returns 0 if the PacketParserResult does not contain a packet.
#[no_mangle]
pub extern "system" fn sq_packet_parser_result_tag<'a>
    (ppr: *mut PacketParserResult<'a>)
    -> c_int
{
    let ppr = ffi_param_ref_mut!(ppr);

    let tag : u8 = match ppr {
        PacketParserResult::Some(ref pp) => pp.packet.tag().into(),
        PacketParserResult::EOF(_) => 0,
    };

    tag as c_int
}

/// If the `PacketParserResult` contains a `PacketParser`, returns it,
/// otherwise, returns NULL.
///
/// If the `PacketParser` reached EOF, then the `PacketParserResult`
/// contains a `PacketParserEOF` and you should use
/// `sq_packet_parser_result_eof` to get it.
///
/// If this function returns a `PacketParser`, then it consumes the
/// `PacketParserResult` and ownership of the `PacketParser` is
/// returned to the caller, i.e., the caller is responsible for
/// ensuring that the `PacketParser` is freed.
#[no_mangle]
pub extern "system" fn sq_packet_parser_result_packet_parser<'a>
    (ppr: *mut PacketParserResult<'a>)
    -> *mut PacketParser<'a>
{
    let ppr = ffi_param_move!(ppr);

    match *ppr {
        PacketParserResult::Some(pp) => box_raw!(pp),
        PacketParserResult::EOF(_) => {
            // Don't free ppr!
            forget(ppr);
            ptr::null_mut()
        }
    }
}

/// If the `PacketParserResult` contains a `PacketParserEOF`, returns
/// it, otherwise, returns NULL.
///
/// If the `PacketParser` did not yet reach EOF, then the
/// `PacketParserResult` contains a `PacketParser` and you should use
/// `sq_packet_parser_result_packet_parser` to get it.
///
/// If this function returns a `PacketParserEOF`, then it consumes the
/// `PacketParserResult` and ownership of the `PacketParserEOF` is
/// returned to the caller, i.e., the caller is responsible for
/// ensuring that the `PacketParserEOF` is freed.
#[no_mangle]
pub extern "system" fn sq_packet_parser_result_eof<'a>
    (ppr: *mut PacketParserResult<'a>)
    -> *mut PacketParserEOF
{
    let ppr = ffi_param_move!(ppr);

    match *ppr {
        PacketParserResult::Some(_) => {
            forget(ppr);
            ptr::null_mut()
        }
        PacketParserResult::EOF(eof) => box_raw!(eof),
    }
}

use self::openpgp::serialize::{
    writer,
    stream::{
        Message,
        Cookie,
        ArbitraryWriter,
        Signer,
        LiteralWriter,
        EncryptionMode,
        Encryptor,
    },
};


/// Streams an OpenPGP message.
#[no_mangle]
pub extern "system" fn sq_writer_stack_message
    (writer: *mut Box<Write>)
     -> *mut writer::Stack<'static, Cookie>
{
    let writer = ffi_param_move!(writer);
    box_raw!(Message::new(writer))
}

/// Writes up to `len` bytes of `buf` into `writer`.
#[no_mangle]
pub extern "system" fn sq_writer_stack_write
    (ctx: *mut Context,
     writer: *mut writer::Stack<'static, Cookie>,
     buf: *const uint8_t, len: size_t)
     -> ssize_t
{
    let ctx = ffi_param_ref_mut!(ctx);
    let writer = ffi_param_ref_mut!(writer);
    assert!(!buf.is_null());
    let buf = unsafe {
        slice::from_raw_parts(buf, len as usize)
    };
    fry_or!(ctx, writer.write(buf).map_err(|e| e.into()), -1) as ssize_t
}

/// Writes up to `len` bytes of `buf` into `writer`.
///
/// Unlike sq_writer_stack_write, unless an error occurs, the whole
/// buffer will be written.  Also, this version automatically catches
/// EINTR.
#[no_mangle]
pub extern "system" fn sq_writer_stack_write_all
    (ctx: *mut Context,
     writer: *mut writer::Stack<'static, Cookie>,
     buf: *const uint8_t, len: size_t)
     -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    let writer = ffi_param_ref_mut!(writer);
    assert!(!buf.is_null());
    let buf = unsafe {
        slice::from_raw_parts(buf, len as usize)
    };
    fry_status!(ctx, writer.write_all(buf).map_err(|e| e.into()))
}

/// Finalizes this writer, returning the underlying writer.
#[no_mangle]
pub extern "system" fn sq_writer_stack_finalize_one
    (ctx: *mut Context,
     writer: *mut writer::Stack<'static, Cookie>)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    if !writer.is_null() {
        let writer = ffi_param_move!(writer);
        maybe_box_raw!(fry!(ctx, writer.finalize_one()))
    } else {
        ptr::null_mut()
    }
}

/// Finalizes all writers, tearing down the whole stack.
#[no_mangle]
pub extern "system" fn sq_writer_stack_finalize
    (ctx: *mut Context,
     writer: *mut writer::Stack<'static, Cookie>)
     -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    if !writer.is_null() {
        let writer = ffi_param_move!(writer);
        fry_status!(ctx, writer.finalize())
    } else {
        Status::Success
    }
}

/// Writes an arbitrary packet.
///
/// This writer can be used to construct arbitrary OpenPGP packets.
/// The body will be written using partial length encoding, or, if the
/// body is short, using full length encoding.
#[no_mangle]
pub extern "system" fn sq_arbitrary_writer_new
    (ctx: *mut Context,
     inner: *mut writer::Stack<'static, Cookie>,
     tag: uint8_t)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    let inner = ffi_param_move!(inner);
    fry_box!(ctx, ArbitraryWriter::new(*inner, tag.into()))
}

/// Signs a packet stream.
///
/// For every signing key, a signer writes a one-pass-signature
/// packet, then hashes and emits the data stream, then for every key
/// writes a signature packet.
#[no_mangle]
pub extern "system" fn sq_signer_new
    (ctx: *mut Context,
     inner: *mut writer::Stack<'static, Cookie>,
     signers: *const &'static TPK, signers_len: size_t)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    let inner = ffi_param_move!(inner);
    let signers = ffi_param_ref!(signers);
    let signers = unsafe {
        slice::from_raw_parts(signers, signers_len)
    };
    fry_box!(ctx, Signer::new(*inner, &signers))
}

/// Creates a signer for a detached signature.
#[no_mangle]
pub extern "system" fn sq_signer_new_detached
    (ctx: *mut Context,
     inner: *mut writer::Stack<'static, Cookie>,
     signers: Option<&&'static TPK>, signers_len: size_t)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    let inner = ffi_param_move!(inner);
    let signers = signers.expect("Signers is NULL");
    let signers = unsafe {
        slice::from_raw_parts(signers, signers_len)
    };
    fry_box!(ctx, Signer::detached(*inner, &signers))
}

/// Writes a literal data packet.
///
/// The body will be written using partial length encoding, or, if the
/// body is short, using full length encoding.
#[no_mangle]
pub extern "system" fn sq_literal_writer_new
    (ctx: *mut Context,
     inner: *mut writer::Stack<'static, Cookie>)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    let inner = ffi_param_move!(inner);
    fry_box!(ctx, LiteralWriter::new(*inner,
                                     DataFormat::Binary,
                                     None,
                                     None))
}

/// Creates a new encryptor.
///
/// The stream will be encrypted using a generated session key,
/// which will be encrypted using the given passwords, and all
/// encryption-capable subkeys of the given TPKs.
///
/// The stream is encrypted using AES256, regardless of any key
/// preferences.
#[no_mangle]
pub extern "system" fn sq_encryptor_new
    (ctx: *mut Context,
     inner: *mut writer::Stack<'static, Cookie>,
     passwords: Option<&*const c_char>, passwords_len: size_t,
     recipients: Option<&&TPK>, recipients_len: size_t,
     encryption_mode: uint8_t)
     -> *mut writer::Stack<'static, Cookie>
{
    let ctx = ffi_param_ref_mut!(ctx);
    let inner = ffi_param_move!(inner);
    let mut passwords_ = Vec::new();
    if passwords_len > 0 {
        let passwords = passwords.expect("Passwords is NULL");
        let passwords = unsafe {
            slice::from_raw_parts(passwords, passwords_len)
        };
        for password in passwords {
            passwords_.push(unsafe {
                CStr::from_ptr(*password)
            }.to_bytes().to_owned().into());
        }
    }
    let recipients = if recipients_len > 0 {
        let recipients = recipients.expect("Recipients is NULL");
        unsafe {
            slice::from_raw_parts(recipients, recipients_len)
        }
    } else {
        &[]
    };
    let encryption_mode = match encryption_mode {
        0 => EncryptionMode::AtRest,
        1 => EncryptionMode::ForTransport,
        _ => panic!("Bad encryption mode: {}", encryption_mode),
    };
    fry_box!(ctx, Encryptor::new(*inner,
                                 &passwords_.iter().collect::<Vec<&Password>>(),
                                 &recipients,
                                 encryption_mode))
}

// Secret.

/// Creates an sq_secret_t from a decrypted session key.
#[no_mangle]
pub fn sq_secret_cached<'a>(algo: u8,
                            session_key: *const u8,
                            session_key_len: size_t)
   -> *mut Secret
{
    let session_key = if session_key_len > 0 {
        unsafe {
            slice::from_raw_parts(session_key, session_key_len)
        }
    } else {
        &[]
    };

    box_raw!(Secret::Cached {
        algo: algo.into(),
        session_key: session_key.to_vec().into()
    })
}


// Decryptor.

/// A message's verification results.
///
/// Conceptually, the verification results are an array of an array of
/// VerificationResult.  The outer array is for the verification level
/// and is indexed by the verification level.  A verification level of
/// zero corresponds to direct signatures; A verification level of 1
/// corresponds to notorizations (i.e., signatures of signatures);
/// etc.
///
/// Within each level, there can be one or more signatures.
pub struct VerificationResults<'a> {
    results: Vec<Vec<&'a VerificationResult>>,
}

/// Returns the `VerificationResult`s at level `level.
///
/// Conceptually, the verification results are an array of an array of
/// VerificationResult.  The outer array is for the verification level
/// and is indexed by the verification level.  A verification level of
/// zero corresponds to direct signatures; A verification level of 1
/// corresponds to notorizations (i.e., signatures of signatures);
/// etc.
///
/// This function returns the verification results for a particular
/// level.  The result is an array of references to
/// `VerificationResult`.
#[no_mangle]
pub fn sq_verification_results_at_level<'a>(results: *const VerificationResults<'a>,
                                            level: size_t,
                                            r: *mut *const &'a VerificationResult,
                                            r_count: *mut size_t) {
    let results = ffi_param_ref!(results);
    let r = ffi_param_ref_mut!(r);
    let r_count = ffi_param_ref_mut!(r_count);

    assert!(level < results.results.len());

    // The size of VerificationResult is not known in C.  Convert from
    // an array of VerificationResult to an array of
    // VerificationResult refs.
    *r = results.results[level].as_ptr();
    *r_count = results.results[level].len();
}

/// Returns the verification result code.
#[no_mangle]
pub fn sq_verification_result_code(result: *const VerificationResult)
    -> c_int
{
    let result = ffi_param_ref!(result);
    match result {
        VerificationResult::GoodChecksum(_) => 1,
        VerificationResult::MissingKey(_) => 2,
        VerificationResult::BadChecksum(_) => 3,
    }
}

/// Returns the verification result code.
#[no_mangle]
pub fn sq_verification_result_signature(result: *const VerificationResult)
    -> *const packet::Signature
{
    let result = ffi_param_ref!(result);
    let sig = match result {
        VerificationResult::GoodChecksum(ref sig) => sig,
        VerificationResult::MissingKey(ref sig) => sig,
        VerificationResult::BadChecksum(ref sig) => sig,
    };

    sig as *const packet::Signature
}

/// Returns the verification result code.
#[no_mangle]
pub fn sq_verification_result_level(result: *const VerificationResult)
    -> c_int
{
    let result = ffi_param_ref!(result);
    result.level() as c_int
}


/// Passed as the first argument to the callbacks used by sq_verify
/// and sq_decrypt.
pub struct HelperCookie {
}

/// How to free the memory allocated by the callback.
type FreeCallback = fn(*mut c_void);

/// Returns the TPKs corresponding to the passed KeyIDs.
///
/// If the free callback is not NULL, then it is called to free the
/// returned array of TPKs.
type GetPublicKeysCallback = fn(*mut HelperCookie,
                                *const &KeyID, usize,
                                &mut *mut &mut TPK, *mut usize,
                                *mut FreeCallback) -> Status;

/// Returns a session key.
type GetSecretKeysCallback = fn(*mut HelperCookie,
                                *const &PKESK, usize,
                                *const &SKESK, usize,
                                &mut *mut Secret) -> Status;

/// Process the signatures.
///
/// If the result is not Status::Success, then this aborts the
/// Verification.
type CheckSignaturesCallback = fn(*mut HelperCookie,
                                  *const VerificationResults,
                                  usize) -> Status;

// This fetches keys and computes the validity of the verification.
struct VHelper {
    get_public_keys_cb: GetPublicKeysCallback,
    check_signatures_cb: CheckSignaturesCallback,
    cookie: *mut HelperCookie,
}

impl VHelper {
    fn new(get_public_keys: GetPublicKeysCallback,
           check_signatures: CheckSignaturesCallback,
           cookie: *mut HelperCookie)
       -> Self
    {
        VHelper {
            get_public_keys_cb: get_public_keys,
            check_signatures_cb: check_signatures,
            cookie: cookie,
        }
    }
}

impl VerificationHelper for VHelper {
    fn get_public_keys(&mut self, ids: &[KeyID])
        -> Result<Vec<TPK>, failure::Error>
    {
        // The size of KeyID is not known in C.  Convert from an array
        // of KeyIDs to an array of KeyID refs.
        let ids : Vec<&KeyID> = ids.iter().collect();

        let mut tpk_refs_raw : *mut &mut TPK = ptr::null_mut();
        let mut tpk_refs_raw_len = 0usize;

        let mut free : FreeCallback = |_| {};

        let result = (self.get_public_keys_cb)(
            self.cookie,
            ids.as_ptr(), ids.len(),
            &mut tpk_refs_raw, &mut tpk_refs_raw_len as *mut usize,
            &mut free);
        if result != Status::Success {
            // XXX: We need to convert the status to an error.  A
            // status contains less information, but we should do the
            // best we can.  For now, we just use
            // Error::InvalidArgument.
            return Err(openpgp::Error::InvalidArgument(
                format!("{:?}", result)).into());
        }

        // Convert the array of references to TPKs to a Vec<TPK>
        // (i.e., not a Vec<&TPK>).
        let mut tpks : Vec<TPK> = Vec::with_capacity(tpk_refs_raw_len);
        for i in 0..tpk_refs_raw_len {
            let tpk = unsafe { ptr::read(*tpk_refs_raw.offset(i as isize)) };
            tpks.push(tpk);
        }

        (free)(tpk_refs_raw as *mut c_void);

        Ok(tpks)
    }

    fn check(&mut self, sigs: Vec<Vec<VerificationResult>>)
        -> Result<(), failure::Error>
    {
        // The size of VerificationResult is not known in C.  Convert
        // from an array of VerificationResults to an array of
        // VerificationResult refs.
        let results = VerificationResults {
            results: sigs.iter().map(
                |r| r.iter().collect::<Vec<&VerificationResult>>()).collect()
        };

        let result = (self.check_signatures_cb)(self.cookie,
                                                &results,
                                                results.results.len());
        if result != Status::Success {
            // XXX: We need to convert the status to an error.  A
            // status contains less information, but we should do the
            // best we can.  For now, we just use
            // Error::InvalidArgument.
            return Err(openpgp::Error::InvalidArgument(
                format!("{:?}", result)).into());
        }

        Ok(())
    }
}

fn verify_real<'a>(input: &'a mut Box<'a + Read>,
                   dsig: Option<&'a mut Box<'a + Read>>,
                   output: Option<&'a mut Box<'a + Write>>,
                   get_public_keys: GetPublicKeysCallback,
                   check_signatures: CheckSignaturesCallback,
                   cookie: *mut HelperCookie)
    -> Result<(), failure::Error>
{
    let h = VHelper::new(get_public_keys, check_signatures, cookie);
    let mut v = if let Some(dsig) = dsig {
        DetachedVerifier::from_reader(dsig, input, h)?
    } else {
        Verifier::from_reader(input, h)?
    };

    let r = if let Some(output) = output {
        io::copy(&mut v, output)
    } else {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match v.read(&mut buffer) {
                // EOF.
                Ok(0) => break Ok(0),
                // Some error.
                Err(err) => break Err(err),
                // Still something to read.
                Ok(_) => continue,
            }
        }
    };

    r.map_err(|e| if e.get_ref().is_some() {
        // Wrapped failure::Error.  Recover it.
        failure::Error::from_boxed_compat(e.into_inner().unwrap())
    } else {
        // Plain io::Error.
        e.into()
    }).context("Verification failed")?;

    Ok(())
}


/// Verifies an OpenPGP message.
///
/// No attempt is made to decrypt any encryption packets.  These are
/// treated as opaque containers.
///
/// Note: output may be NULL, if the output is not required.
#[no_mangle]
pub fn sq_verify<'a>(ctx: *mut Context,
                     input: *mut Box<'a + Read>,
                     dsig: Option<&'a mut Box<'a + Read>>,
                     output: Option<&'a mut Box<'a + Write>>,
                     get_public_keys: GetPublicKeysCallback,
                     check_signatures: CheckSignaturesCallback,
                     cookie: *mut HelperCookie)
    -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    let input = ffi_param_ref_mut!(input);

    let r = verify_real(input, dsig, output,
        get_public_keys, check_signatures, cookie);

    fry_status!(ctx, r)
}


struct DHelper {
    vhelper: VHelper,
    get_secret_keys_cb: GetSecretKeysCallback,
}

impl DHelper {
    fn new(get_public_keys: GetPublicKeysCallback,
           get_secret_keys: GetSecretKeysCallback,
           check_signatures: CheckSignaturesCallback,
           cookie: *mut HelperCookie)
       -> Self
    {
        DHelper {
            vhelper: VHelper::new(get_public_keys, check_signatures, cookie),
            get_secret_keys_cb: get_secret_keys,
        }
    }
}

impl VerificationHelper for DHelper {
    fn get_public_keys(&mut self, ids: &[KeyID])
        -> Result<Vec<TPK>, failure::Error>
    {
        self.vhelper.get_public_keys(ids)
    }

    fn check(&mut self, sigs: Vec<Vec<VerificationResult>>)
        -> Result<(), failure::Error>
    {
        self.vhelper.check(sigs)
    }
}

impl DecryptionHelper for DHelper {
    fn get_secret(&mut self, pkesks: &[&PKESK], skesks: &[&SKESK])
        -> Result<Option<Secret>, failure::Error>
    {
        let mut secret : *mut Secret = ptr::null_mut();

        let result = (self.get_secret_keys_cb)(
            self.vhelper.cookie,
            pkesks.as_ptr(), pkesks.len(), skesks.as_ptr(), skesks.len(),
            &mut secret);
        if result != Status::Success {
            // XXX: We need to convert the status to an error.  A
            // status contains less information, but we should do the
            // best we can.  For now, we just use
            // Error::InvalidArgument.
            return Err(openpgp::Error::InvalidArgument(
                format!("{:?}", result)).into());
        }

        if secret.is_null() {
            return Err(openpgp::Error::MissingSessionKey(
                "Callback did not return a session key".into()).into());
        }

        let secret = ffi_param_move!(secret);

        Ok(Some(*secret))
    }
}

// A helper function that returns a Result so that we can use ? to
// propagate errors.
fn decrypt_real<'a>(input: &'a mut Box<'a + Read>,
                    output: &'a mut Box<'a + Write>,
                    get_public_keys: GetPublicKeysCallback,
                    get_secret_keys: GetSecretKeysCallback,
                    check_signatures: CheckSignaturesCallback,
                    cookie: *mut HelperCookie)
    -> Result<(), failure::Error>
{
    let helper = DHelper::new(
        get_public_keys, get_secret_keys, check_signatures, cookie);

    let mut decryptor = Decryptor::from_reader(input, helper)
        .context("Decryption failed")?;

    io::copy(&mut decryptor, output)
        .map_err(|e| if e.get_ref().is_some() {
            // Wrapped failure::Error.  Recover it.
            failure::Error::from_boxed_compat(e.into_inner().unwrap())
        } else {
            // Plain io::Error.
            e.into()
        }).context("Decryption failed")?;

    Ok(())
}

/// Decrypts an OpenPGP message.
///
/// The message is read from `input` and the content of the
/// `LiteralData` packet is written to output.  Note: the content is
/// written even if the message is not encrypted.  You can determine
/// whether the message was actually decrypted by recording whether
/// the get_secret_keys callback was called in the cookie.
///
/// The function takes three callbacks.  The `cookie` is passed as the
/// first parameter to each of them.
///
/// Note: all of the parameters are required; none may be NULL.
#[no_mangle]
pub fn sq_decrypt<'a>(ctx: *mut Context,
                      input: *mut Box<'a + Read>,
                      output: *mut Box<'a + Write>,
                      get_public_keys: GetPublicKeysCallback,
                      get_secret_keys: GetSecretKeysCallback,
                      check_signatures: CheckSignaturesCallback,
                      cookie: *mut HelperCookie)
    -> Status
{
    let ctx = ffi_param_ref_mut!(ctx);
    let input = ffi_param_ref_mut!(input);
    let output = ffi_param_ref_mut!(output);

    let r = decrypt_real(input, output,
        get_public_keys, get_secret_keys, check_signatures, cookie);

    fry_status!(ctx, r)
}
