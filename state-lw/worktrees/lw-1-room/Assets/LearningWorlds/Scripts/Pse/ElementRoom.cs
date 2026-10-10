using UnityEngine;
using System.Collections.Generic;

public static class ElementRoom
{
    public static GameObject Build(Element selectedElement)
    {
        // --- Root Object ---
        GameObject roomRoot = new GameObject("ElementRoom" + selectedElement.Symbol);
        roomRoot.transform.position = Vector3.zero;

        // --- Floor Mandala: Three Translucent Discs (Category Color)
        Transform mandalaRoot = new GameObject("MandalaDiscs").transform;
        mandalaRoot.SetParent(roomRoot.transform, false);
        Color categoryColor = selectedElement.CategoryColor;
        float discRadius = 1.5f;
        float discSpacing = 0.1f;

        for (int i = 0; i < 3; i++)
        {
            GameObject disc = GameObject.CreatePrimitive(PrimitiveType.Cylinder);
            disc.transform.localScale = new Vector3(discRadius, 0.05f, discRadius);
            disc.transform.position = new Vector3(i * discSpacing, -discRadius / 2, -discRadius);
            disc.transform.SetParent(mandalaRoot, false);
            disc.GetComponent<Renderer>().material.color = categoryColor;
            disc.GetComponent<Renderer>().material.SetInt("_SrcBlend", (int)UnityEngine.Rendering.BlendMode.SrcAlpha);
            disc.GetComponent<Renderer>().material.SetInt("_DstBlend", (int)UnityEngine.Rendering.BlendMode.OneMinusSrcAlpha);
            disc.GetComponent<Renderer>().material.SetInt("_ZWrite", 0);
            disc.GetComponent<Renderer>().material.shader = Shader.Find("Standard");
            disc.name = "Disc" + i;
        }

        // --- Giant Bohr Atom Monument ---
        GameObject boh Atom = BohrAtomBuilder.Build(selectedElement);
        bohAtom.transform.localScale *= 2.5f; // radiusScale
        bohAtom.transform.SetParent(roomRoot.transform, false);

        // --- Symbol + Name on Wall (TextMesh) ---
        GameObject symbolTextObject = new GameObject("Symbol");
        symbolTextObject.transform.SetParent(roomRoot.transform, false);
        TextMesh symbolText = symbolTextObject.AddComponent<TextMesh>();
        symbolText.text = $"{selectedElement.Symbol}\n{TextUtil.Wrap(selectedElement.Name)}";
        symbolText align = TextAnchor.UpperLeft;
        symbolText.anchor = TextAnchor.UpperCenter;
        symbolText.fontSize = 24;
        symbolText.transform.position = new Vector3(0, 0.5f, -4);
        symbolTextObject.transform.SetParent(roomRoot.transform, false);

        // --- Ring of 5 Learning Stations ---
        GameObject stations = new GameObject("LearningStations");
        stations.transform.SetParent(roomRoot.transform, false);
        stations.transform.Rotate(0, 180, 0, Space.Self);

        float stationRadius = 3.5f;
        float stationSpacing = 360f / 5;

        for (int i = 0; i < 5; i++)
        {
            float angle = i * stationSpacing * Mathf.Deg2Rad;
            Vector3 stationPosition = new Vector3(
                Mathf.Cos(angle) * stationRadius,
                0,
                Mathf.Sin(angle) * stationRadius
            );

            GameObject stationGO = new GameObject("Station_" + (i + 1));
            stationGO.transform.position = stationPosition;
            stationGO.transform.SetParent(stations.transform, false);

            // Add a Post
            float postHeight = 1.5f;
            GameObject post = GameObject.CreatePrimitive(PrimitiveType.Cylinder);
            post.transform.parent = stationGO.transform;
            post.transform.localScale = new Vector3(0.3f, postHeight, 0.3f);
            post.transform.localPosition = new Vector3(0, postHeight / 2, 0);
            post.name = "Post_" + i;

            // Add Learning Station Labeling Network Object
            GameObject label = new GameObject("Label_" + i);
            label.transform.parent = stationGO.transform;
            label.transform.localPosition = new Vector3(0, postHeight + 0.1f, 0);
            TextMesh stationText = label.AddComponent<TextMesh>();
            stationText.text = GetStationLabel(i + 1);
            stationText.align = TextAnchor.MiddleCenter;
            stationText.fontSize = 14;
            stationText.transform.localRotation = Quaternion.Euler(90, 0, 0);
        }

        // --- Return Portal ---
        GameObject returnPortal = GameObject.CreatePrimitive(PrimitiveType.Cylinder);
        returnPortal.transform.parent = roomRoot.transform;
        returnPortal.transform.position = new Vector3(-4, 0, 0);
        returnPortal.transform.localScale = new Vector3(0.5f, 0.1f, 0.5f);
        returnPortal.GetComponent<Renderer>().material.color = Color.blue;
        returnPortal.name = "PortalToPseGallery";
        returnPortal.AddComponent< Portal>().targetWorld = World.PseGallery;

        BuildingBuilders.RegisterBuilder(World.ElementRoom, Build);
        return roomRoot;
    }

    private static string GetStationLabel(int stationId)
    {
        switch (stationId)
        {
            case 1: return "1-Kristallstruktur";
            case 2: return "2-Wo du ihm begegnest";
            case 3: return "3-Entdeckung";
            case 4: return "4-Klassisches Experiment";
            case 5: return "5-Quiz";
            default: return "Learning Station";
        }
    }

    // Portal class: required for transitions
    public class Portal : MonoBehaviour
    {
        public World targetWorld;
    }
}