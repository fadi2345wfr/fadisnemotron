package de.fadi.nemotron.ui.common

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import de.fadi.nemotron.R

/** A photo or video waiting in the composer, with a button to remove it. */
@Composable
fun AttachmentChip(attachment: Attachment, removable: Boolean, onRemove: () -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(CHIP_SIZE.dp)) {
            Box(Modifier.fillMaxSize().clip(RoundedCornerShape(8.dp)).background(Color.Black)) {
                when (attachment) {
                    is PhotoAttachment -> Preview(attachment.photo.thumbnail, attachment.name)
                    is VideoAttachment -> VideoPreview(attachment)
                }
            }
            if (removable) {
                Surface(
                    onClick = onRemove,
                    shape = CircleShape,
                    color = MaterialTheme.colorScheme.inverseSurface,
                    modifier = Modifier.align(Alignment.TopEnd).padding(2.dp).size(22.dp),
                ) {
                    Icon(
                        painterResource(R.drawable.ic_close),
                        contentDescription = stringResource(R.string.remove_attachment, attachment.name),
                        tint = MaterialTheme.colorScheme.inverseOnSurface,
                        modifier = Modifier.padding(3.dp),
                    )
                }
            }
        }
        if (attachment is VideoAttachment && attachment.upload == null) {
            Text(
                stringResource(R.string.video_shrinking, attachment.progress),
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(start = 8.dp).widthIn(max = 120.dp),
            )
        }
    }
}

@Composable
private fun VideoPreview(video: VideoAttachment) {
    Box(Modifier.fillMaxSize()) {
        video.thumbnail?.let { Preview(it, video.name) }
        if (video.upload == null) {
            Box(Modifier.fillMaxSize().background(SCRIM))
            CircularProgressIndicator(
                progress = { video.progress / 100f },
                color = Color.White,
                trackColor = Color.White.copy(alpha = 0.3f),
                strokeWidth = 3.dp,
                modifier = Modifier.align(Alignment.Center).size(28.dp),
            )
        } else {
            Icon(
                painterResource(R.drawable.ic_play),
                contentDescription = null,
                tint = Color.White,
                modifier = Modifier.align(Alignment.Center).size(28.dp).background(SCRIM, CircleShape).padding(2.dp),
            )
        }
        Text(
            formatDuration(video.durationMs),
            style = MaterialTheme.typography.labelSmall,
            color = Color.White,
            modifier = Modifier
                .align(Alignment.BottomStart)
                .padding(4.dp)
                .background(SCRIM, RoundedCornerShape(4.dp))
                .padding(horizontal = 4.dp),
        )
    }
}

@Composable
private fun Preview(bitmap: Bitmap, description: String) {
    val image = remember(bitmap) { bitmap.asImageBitmap() }
    Image(image, contentDescription = description, contentScale = ContentScale.Crop, modifier = Modifier.fillMaxSize())
}

private const val CHIP_SIZE = 64
private val SCRIM = Color.Black.copy(alpha = 0.5f)
