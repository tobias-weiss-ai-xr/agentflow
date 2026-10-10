using System.Collections.Generic;
using UnityEngine;

public static class LatticeBuilder
{ foglal{
    public static GameObject BuildStructure(Element element, string symbol, string category, Vector3 center, float cell = 0.3f)
    {
        GameObject latticeRoot = new GameObject(element.Symbol + "_Lattice");
        latticeRoot.transform.position = center;

        // Determine lattice type based on element
        GameObject atoms = CreateLatticeAtoms(element, latticeRoot, cell);
        CreateLatticeBonds(element, atoms);

        return latticeRoot;
    }

    private static GameObject CreateLatticeAtoms(Element element, GameObject parent, float cellSize)
    {
        // Create container for atom locations
        GameObject atomGroup = new GameObject(element.Symbol + "_Atoms");
        atomGroup.transform.SetParent(parent.transform, false);

        // Element-dependent structure logic
        List<Vector3> atomPositions = GetAtomPositions(element);

        for (int i = 0; i < atomPositions.Count; i++)
        {
            Vector3 atomPos = atomPositions[i];
            GameObject atom = GameObject.CreatePrimitive(PrimitiveType.Sphere);
            atom.transform.SetParent(atomGroup.transform);
            atom.transform.localPosition = cellSize * atomPos;
            atom.transform.localScale = Vector3.one * cellSize * 0.3f;
            atom.GetComponent<Renderer>().material.color = element.AtomColor;
            atom.name = "Atom_" + i;
        }

        return atomGroup;
    }

    private static List<Vector3> GetAtomPositions(Element element)
    {
        List<Vector3> positions = new List<Vector3>();

        if (IsBCC(element))
        {
            // Body-centered cubic lattice
            for (int x = 0; x < 3; x++)
            {
                for (int y = 0; y < 3; y++)
                {
                    for (int z = 0; z < 3; z++)
                    {
                        positions.Add(new Vector3(x - 1, y - 1, z - 1));
                        // Center atom
                        positions.Add(new Vector3(x + 0.5f - 1, y + 0.5f - 1, z + 0.5f - 1));
                    }
                }
            }
        }
        else if (IsHCP(element))
        {
            // Hexagonal close-packed lattice
            for (int x = 0; x < 3; x++)
            {
                for (int y = 0; y < 3; y++)
                {
                    for (int z = 0; z < 2; z++)
                    {
                        // A sites
                        positions.Add(new Vector3(x - 1, y - 1 + (z % 2) * 0.5f, Mathf.Sqrt(3) * z));
                        // B sites
                        positions.Add(new Vector3(x + 0.5f - 1, y + 0.33f - 1 + (z % 2) * 0.5f, Mathf.Sqrt(3) * z));
                    }
                }
            }
        }
        else if (IsDiamond(element))
        {
            // Diamond cubic lattice
            for (int x = 0; x < 3; x++)
            {
                for (int y = 0; y < 3; y++)
                {
                    for (int z = 0; z < 3; z++)
                    {
                        // FCC positions
                        positions.Add(new Vector3(x - 1, y - 1, z - 1));
                        positions.Add(new Vector3(x + 0.5f - 1, y + 0.5f - 1, z + 0.5f - 1));
                        positions.Add(new Vector3(x + 0.5f - 1, y - 0.5f - 1, z - 0.5f - 1));
                        positions.Add(new Vector3(x - 0.5f - 1, y + 0.5f - 1, z - 0.5f - 1));
                    }
                }
            }
        }
        else
        {
            // Default Face-centered cubic
            for (int x = 0; x < 3; x++)
            {
                for (int y = 0; y < 3; y++)
                {
                    for (int z = 0; z < 3; z++)
                    {
                        positions.Add(new Vector3(x - 1, y - 1, z - 1));
                    }
                }
                // Add center points for FCC
                for (int x = 0; x < 3; x++)
                {
                    for (int y = 0; y < 3; y++)
                    {
                        positions.Add(new Vector3(x + 0.5f - 1, y + 0.5f - 1, -1));
                        positions.Add(new Vector3(x + 0.5f - 1, -0.5f - 1, y + 0.5f - 1));
                        positions.Add(new Vector3(-0.5f - 1, x + 0.5f - 1, y + 0.5f - 1));
                    }
                }
            }
        }

        return positions;
    }

    private static bool IsBCC(Element element)
    {
        return element.Symbol == "Fe" || element.Symbol == "Cr" || element.Symbol == "W" ||
               element.Symbol == "V" || element.Symbol == "Mo" || element.Symbol == "Ta" ||
               element.Symbol == "Nb";
    }

    private static bool IsHCP(Element element)
    {
        return element.Symbol == "Co" || element.Symbol == "Zn" || element.Symbol == "Mg" ||
               element.Symbol == "Ti";
    }

    private static bool IsDiamond(Element element)
    {
        return element.Symbol == "C";
    }

    private static void CreateLatticeBonds(Element element, GameObject atomsRoot)
    {
        GameObject bondsRoot = new GameObject(element.Symbol + "_Bonds");
        bondsRoot.transform.SetParent(atomsRoot.transform);

        Transform[] atomTransforms = atomsRoot.GetComponentsInChildren<Transform>();

        for (int i = 0; i < atomTransforms.Length; i++)
        {
            Transform atomA = atomTransforms[i];

            for (int j = i + 1; j < atomTransforms.Length; j++)
            {
                Transform atomB = atomTransforms[j];

                float distance = Vector3.Distance(atomA.localPosition, atomB.localPosition);
                if (distance <= 0.4f) // Max lattice spacing threshold
                {
                    GameObject line = new GameObject("Bond");
                    line.transform.SetParent(bondsRoot.transform, false);

                    LineRenderer lr = line.AddComponent<LineRenderer>();
                    lr.startWidth = 0.02f;
                    lr.endWidth = 0.02f;
                    lr.material = new Material(Shader.Find("Unlit/Color"));
                    lr.material.color = Color.gray;
                    lr.SetPosition(0, atomA.localPosition);
                    lr.SetPosition(1, atomB.localPosition);
                }
            }
        }
    }
}